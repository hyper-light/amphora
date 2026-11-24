//! `pg_catalog`'s relations as the PostgreSQL dialect serves them (Amphora's
//! docs/design/postgres.md section 9): PostgreSQL 19's built-in rows (`amphora-pg-catalog`), and
//! rows derived at each scan from the tables' stored definitions, never copied.
//!
//! A user table's rows are PostgreSQL's own rows for its catalogs, as bootstrap wrote them, with the
//! table's facts in place of the catalog's: its OID, name, schema and columns.

use crate::dialect::postgres::{split_stored, type_modifier};
use crate::schema::BTreeTable;
use crate::sync::Arc;
use crate::sync::RwLock;
use crate::vtab::{InternalVirtualTable, InternalVirtualTableCursor};
use crate::{Connection, LimboError, Result, Value};
use amphora_engine_ext::{ConstraintInfo, ConstraintUsage, IndexInfo, OrderByInfo, ResultCode};
use amphora_pg_catalog::data::{
    PG_ATTRIBUTE, PG_CLASS, PG_INHERITS, PG_NAMESPACE, PG_PARTITIONED_TABLE, PG_TYPE,
};
use amphora_pg_catalog::{Catalog, Datum};

/// The `pg_catalog` relations served so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PgCatalog {
    Namespace,
    Type,
    Class,
    Attribute,
    /// Empty: there are no partitions yet, nor inheritance.
    PartitionedTable,
    Inherits,
}

impl PgCatalog {
    pub const ALL: [Self; 6] = [
        Self::Namespace,
        Self::Type,
        Self::Class,
        Self::Attribute,
        Self::PartitionedTable,
        Self::Inherits,
    ];

    fn catalog(self) -> &'static Catalog {
        match self {
            Self::Namespace => &PG_NAMESPACE,
            Self::Type => &PG_TYPE,
            Self::Class => &PG_CLASS,
            Self::Attribute => &PG_ATTRIBUTE,
            Self::PartitionedTable => &PG_PARTITIONED_TABLE,
            Self::Inherits => &PG_INHERITS,
        }
    }
}

#[derive(Debug)]
pub struct PgCatalogTable(PgCatalog);

impl PgCatalogTable {
    pub fn new(catalog: PgCatalog) -> Self {
        Self(catalog)
    }
}

/// The storage class a column of PostgreSQL type `oid` takes (section 5); none for the types no
/// built-in row holds a value of (arrays, `pg_node_tree`), so nothing converts them.
fn storage_class(oid: u32) -> &'static str {
    match oid {
        // bool, int8, int2, int4, regproc, oid, xid, cid
        16 | 20 | 21 | 23 | 24 | 26 | 28 | 29 => " INTEGER",
        // float4, float8
        700 | 701 => " REAL",
        // "char", name, text
        18 | 19 | 25 => " TEXT",
        _ => "",
    }
}

impl InternalVirtualTable for PgCatalogTable {
    fn name(&self) -> String {
        self.0.catalog().name.to_string()
    }

    fn sql(&self) -> String {
        let catalog = self.0.catalog();
        let columns: Vec<String> = catalog
            .columns
            .iter()
            .map(|c| format!("{}{}", c.name, storage_class(c.type_oid)))
            .collect();
        format!("CREATE TABLE {}({})", catalog.name, columns.join(", "))
    }

    fn open(&self, conn: Arc<Connection>) -> Result<Arc<RwLock<dyn InternalVirtualTableCursor>>> {
        Ok(Arc::new(RwLock::new(PgCatalogCursor {
            conn,
            catalog: self.0,
            rows: Vec::new(),
            index: 0,
        })))
    }

    fn best_index(
        &self,
        constraints: &[ConstraintInfo],
        _order_by: &[OrderByInfo],
    ) -> std::result::Result<IndexInfo, ResultCode> {
        let built_in = self.0.catalog().rows.len();
        Ok(IndexInfo {
            idx_num: 0,
            idx_str: None,
            order_by_consumed: false,
            estimated_cost: built_in as f64,
            estimated_rows: u32::try_from(built_in).unwrap_or(u32::MAX),
            constraint_usages: constraints
                .iter()
                .map(|_| ConstraintUsage {
                    argv_index: None,
                    omit: false,
                })
                .collect(),
        })
    }
}

pub struct PgCatalogCursor {
    conn: Arc<Connection>,
    catalog: PgCatalog,
    /// The rows of this scan, read when it starts.
    rows: Vec<Vec<Value>>,
    index: usize,
}

impl InternalVirtualTableCursor for PgCatalogCursor {
    fn filter(&mut self, _args: &[Value], _idx_str: Option<String>, _idx_num: i32) -> Result<bool> {
        self.rows = rows(self.catalog, &self.conn)?;
        self.index = 0;
        Ok(!self.rows.is_empty())
    }

    fn next(&mut self) -> Result<bool> {
        // Past the end stays past the end.
        self.index = self.index.saturating_add(1);
        Ok(self.index < self.rows.len())
    }

    fn column(&self, column: usize) -> Result<Value> {
        Ok(self
            .rows
            .get(self.index)
            .and_then(|row| row.get(column))
            .cloned()
            .unwrap_or(Value::Null))
    }

    fn rowid(&self) -> i64 {
        i64::try_from(self.index).map_or(i64::MAX, |i| i.saturating_add(1))
    }
}

/// A value as the engine stores its PostgreSQL type (section 5).
pub(crate) fn value(datum: &Datum) -> Value {
    match *datum {
        Datum::Null => Value::Null,
        Datum::Bool(b) => Value::from_i64(i64::from(b)),
        Datum::Char(byte) => Value::from_text(char_text(byte)),
        Datum::Int2(v) => Value::from_i64(i64::from(v)),
        Datum::Int4(v) => Value::from_i64(i64::from(v)),
        Datum::Float4(v) => Value::from_f64(f64::from(v)),
        Datum::Oid(v) | Datum::Xid(v) => Value::from_i64(i64::from(v)),
        Datum::Name(s) | Datum::Text(s) => Value::from_text(s.to_string()),
    }
}

/// `"char"` as `charout` prints it (src/backend/utils/adt/char.c): nothing for a zero byte, an
/// ASCII byte as itself, any other as a backslash and three octal digits.
fn char_text(byte: u8) -> String {
    match byte {
        0 => String::new(),
        b if b.is_ascii() => char::from(b).to_string(),
        b => format!("\\{b:03o}"),
    }
}

fn rows(catalog: PgCatalog, conn: &Arc<Connection>) -> Result<Vec<Vec<Value>>> {
    let mut rows: Vec<Vec<Value>> = catalog
        .catalog()
        .rows
        .iter()
        .map(|row| row.iter().map(value).collect())
        .collect();
    match catalog {
        PgCatalog::Namespace
        | PgCatalog::Type
        | PgCatalog::PartitionedTable
        | PgCatalog::Inherits => {}
        PgCatalog::Class => {
            for table in user_tables(conn)? {
                rows.push(class_row(&table)?);
            }
        }
        PgCatalog::Attribute => {
            for table in user_tables(conn)? {
                rows.extend(attribute_rows(&table)?);
            }
        }
    }
    Ok(rows)
}

/// A table of the connection's main schema with a `pg_class` OID.
struct UserTable {
    oid: u32,
    table: BTreeTable,
    has_index: bool,
    has_trigger: bool,
}

/// The connection's tables, read from `sqlite_schema` as its transaction sees it.
fn user_tables(conn: &Arc<Connection>) -> Result<Vec<UserTable>> {
    let mut stmt =
        conn.prepare_internal("SELECT type, tbl_name, rootpage, sql FROM sqlite_schema")?;
    let entries = stmt.run_collect_rows()?;
    let text = |v: Option<&Value>| match v {
        Some(Value::Text(t)) => Some(t.as_str().to_string()),
        _ => None,
    };
    let owns = |kind: &str, name: &str| {
        entries.iter().any(|e| {
            text(e.first()).as_deref() == Some(kind)
                && text(e.get(1)).is_some_and(|owner| owner.eq_ignore_ascii_case(name))
        })
    };
    let mut tables = Vec::new();
    for entry in &entries {
        if text(entry.first()).as_deref() != Some("table") {
            continue;
        }
        let Some(sql) = text(entry.get(3)) else {
            continue;
        };
        let Some((oid, definition)) = split_stored(&sql)? else {
            continue;
        };
        let root_page = match entry.get(2) {
            Some(Value::Numeric(crate::Numeric::Integer(page))) => *page,
            _ => 0,
        };
        let table = BTreeTable::from_sql(definition, root_page)?;
        // PostgreSQL indexes every primary key, and implements foreign keys as triggers.
        let has_index = owns("index", &table.name) || !table.primary_key_columns.is_empty();
        let has_trigger = owns("trigger", &table.name) || !table.foreign_keys.is_empty();
        tables.push(UserTable {
            oid,
            table,
            has_index,
            has_trigger,
        });
    }
    Ok(tables)
}

/// A built-in row of `catalog` whose `column` holds `key`: the template a user row starts from.
fn template(
    catalog: &'static Catalog,
    column: &str,
    key: Datum<'static>,
    also: impl Fn(&[Datum<'static>]) -> bool,
) -> Result<&'static [Datum<'static>]> {
    let at = catalog.column(column);
    catalog
        .rows
        .iter()
        .copied()
        .find(|row| at.and_then(|i| row.get(i)) == Some(&key) && also(row))
        .ok_or_else(|| LimboError::InternalError(format!("{} has no template row", catalog.name)))
}

/// `template` with the values `own` gives in place of its own.
fn derived<'a>(
    catalog: &Catalog,
    template: &[Datum<'static>],
    own: impl Fn(&str) -> Option<Datum<'a>>,
) -> Vec<Value> {
    catalog
        .columns
        .iter()
        .zip(template)
        .map(|(column, built_in)| value(&own(column.name).unwrap_or(*built_in)))
        .collect()
}

/// `public`'s OID (src/include/catalog/pg_namespace.dat), where the main schema's tables live.
fn public_namespace() -> Result<u32> {
    let row = template(&PG_NAMESPACE, "nspname", Datum::Name("public"), |_| true)?;
    match PG_NAMESPACE.column("oid").and_then(|i| row.get(i)) {
        Some(Datum::Oid(oid)) => Ok(*oid),
        _ => Err(LimboError::InternalError(
            "pg_namespace has no public".into(),
        )),
    }
}

/// The columns PostgreSQL shows: hidden companion columns (section 5) are the engine's own.
fn visible_columns(table: &BTreeTable) -> impl Iterator<Item = &crate::schema::Column> {
    table.columns().iter().filter(|c| !c.hidden())
}

fn class_row(t: &UserTable) -> Result<Vec<Value>> {
    let as_int2 = |n: usize| {
        i16::try_from(n)
            .map_err(|_| LimboError::SqlError("tables can have at most 1600 columns".into()))
    };
    let natts = as_int2(visible_columns(&t.table).count())?;
    let checks = as_int2(t.table.check_constraints.len())?;
    let namespace = public_namespace()?;
    // pg_class's own row is the plain heap table bootstrap wrote: owner, access method, empty
    // statistics, permanent, ordinary.
    let base = template(&PG_CLASS, "oid", Datum::Oid(PG_CLASS.oid), |_| true)?;
    Ok(derived(&PG_CLASS, base, |column| match column {
        "oid" | "relfilenode" => Some(Datum::Oid(t.oid)),
        "relname" => Some(Datum::Name(&t.table.name)),
        "relnamespace" => Some(Datum::Oid(namespace)),
        // The table's row type is not made yet.
        "reltype" => Some(Datum::Oid(0)),
        "relhasindex" => Some(Datum::Bool(t.has_index)),
        "relnatts" => Some(Datum::Int2(natts)),
        "relchecks" => Some(Datum::Int2(checks)),
        "relhastriggers" => Some(Datum::Bool(t.has_trigger)),
        // REPLICA_IDENTITY_DEFAULT (src/include/catalog/pg_class.h); catalogs have 'n'.
        "relreplident" => Some(Datum::Char(b'd')),
        _ => None,
    }))
}

fn attribute_rows(t: &UserTable) -> Result<Vec<Vec<Value>>> {
    let attrelid = PG_ATTRIBUTE.column("attrelid");
    let attnum = PG_ATTRIBUTE.column("attnum");
    let of_pg_class = |row: &&&[Datum<'static>]| {
        attrelid.and_then(|i| row.get(i)) == Some(&Datum::Oid(PG_CLASS.oid))
    };
    let negative = |row: &[Datum<'static>]| matches!(attnum.and_then(|i| row.get(i)), Some(Datum::Int2(n)) if *n < 0);
    // Every table has the system columns pg_class has (ctid, xmin, cmin, xmax, cmax, tableoid).
    let mut rows: Vec<Vec<Value>> = PG_ATTRIBUTE
        .rows
        .iter()
        .filter(of_pg_class)
        .filter(|row| negative(row))
        .map(|row| {
            derived(&PG_ATTRIBUTE, row, |c| {
                if c == "attrelid" {
                    Some(Datum::Oid(t.oid))
                } else {
                    None
                }
            })
        })
        .collect();
    // pg_class's `relname` is a plain, local, undropped column: the template for a user column.
    let base = template(&PG_ATTRIBUTE, "attname", Datum::Name("relname"), |row| {
        attrelid.and_then(|i| row.get(i)) == Some(&Datum::Oid(PG_CLASS.oid))
    })?;
    for (number, column) in visible_columns(&t.table).enumerate() {
        let name = column.name.as_deref().unwrap_or_default();
        let modifiers = column
            .ty_params
            .iter()
            .map(|p| type_modifier(p))
            .collect::<Result<Vec<i64>>>()?;
        let ty = amphora_pg_catalog::column_type(&column.ty_str, &modifiers)
            .map_err(|e| LimboError::SqlError(e.to_string()))?;
        let type_row = template(&PG_TYPE, "oid", Datum::Oid(ty.oid), |_| true)?;
        let of_type = |field: &str| PG_TYPE.column(field).and_then(|i| type_row.get(i)).copied();
        let attnum = number
            .checked_add(1)
            .and_then(|n| i16::try_from(n).ok())
            .ok_or_else(|| LimboError::SqlError("tables can have at most 1600 columns".into()))?;
        let generated = if column.is_virtual_generated() {
            b'v'
        } else if column.is_generated() {
            b's'
        } else {
            0
        };
        rows.push(derived(&PG_ATTRIBUTE, base, |field| match field {
            "attrelid" => Some(Datum::Oid(t.oid)),
            "attname" => Some(Datum::Name(name)),
            "atttypid" => Some(Datum::Oid(ty.oid)),
            "attlen" => of_type("typlen"),
            "attnum" => Some(Datum::Int2(attnum)),
            "atttypmod" => Some(Datum::Int4(ty.typmod)),
            "attbyval" => of_type("typbyval"),
            "attalign" => of_type("typalign"),
            "attstorage" => of_type("typstorage"),
            "attcollation" => of_type("typcollation"),
            // A primary key's columns are NOT NULL in PostgreSQL.
            "attnotnull" => Some(Datum::Bool(column.notnull() || column.primary_key())),
            "atthasdef" => Some(Datum::Bool(column.default.is_some())),
            "attgenerated" => Some(Datum::Char(generated)),
            _ => None,
        }));
    }
    Ok(rows)
}
