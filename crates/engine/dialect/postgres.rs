//! The PostgreSQL dialect (Amphora's docs/design/postgres.md).
//!
//! A table's stored definition is [`super::POSTGRES_TABLE_SQL_PREFIX`], then its `pg_class` OID,
//! then its definition: the OID lives in the one durable copy of the table, so it is created,
//! renamed and dropped with the table, and stays the same across a reopen (section 9). The
//! `pg_catalog` relations it registers are served from the schema
//! ([`crate::pg_catalog_vtab`]).
//!
//! Until the PostgreSQL front end lowers its statements into the engine (section 11's units 8 and
//! 9), this dialect reads the engine's canonical text, which every dialect must accept.

use crate::function::Func;
use crate::pg_catalog_vtab::{PgCatalog, PgCatalogTable};
use crate::schema::{BTreeTable, Schema};
use crate::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use crate::sync::Arc;
use crate::{Connection, LimboError, Value};
use amphora_engine_parser::ast;

/// What follows the prefix in a stored definition: `/* pg_class.oid <oid> */ `.
const OID_OPEN: &str = "/* pg_class.oid ";
const OID_CLOSE: &str = " */ ";

/// Next OID in the normal PostgreSQL OID ring `[FIRST_NORMAL_OBJECT_ID ..= u32::MAX]`, wrapping
/// from `u32::MAX` directly back to `FIRST_NORMAL_OBJECT_ID` so the durable header marker is
/// never cleared to `0` across wraparound.
fn next_normal_oid(oid: u32) -> u32 {
    let next = oid.wrapping_add(1);
    if next < amphora_pg_catalog::FIRST_NORMAL_OBJECT_ID {
        amphora_pg_catalog::FIRST_NORMAL_OBJECT_ID
    } else {
        next
    }
}

/// True when `candidate` is strictly ahead of `current` in the circular normal OID ring
/// `[FIRST_NORMAL_OBJECT_ID ..= u32::MAX]` (forward modular distance in `(0, ring / 2)`),
/// correctly handling wraparound from `u32::MAX` to `FIRST_NORMAL_OBJECT_ID`.
fn oid_is_ahead(candidate: u32, current: u32) -> bool {
    let first = u64::from(amphora_pg_catalog::FIRST_NORMAL_OBJECT_ID);
    let ring = (u64::from(u32::MAX) - first) + 1;
    let c = u64::from(candidate.max(amphora_pg_catalog::FIRST_NORMAL_OBJECT_ID)) - first;
    let cur = u64::from(current.max(amphora_pg_catalog::FIRST_NORMAL_OBJECT_ID)) - first;
    let dist = (c + ring - cur) % ring;
    dist > 0 && dist < (ring / 2)
}

/// The PostgreSQL dialect.
pub struct PostgresDialect {
    /// PostgreSQL's `TransamVariables->nextOid` (REL_19_0 src/backend/access/transam/varsup.c:554):
    /// in memory and outside every transaction, so a rolled-back CREATE TABLE spends its OID while
    /// the database is open, as PostgreSQL's does. Its durable copy is the header's `pg_next_oid`,
    /// which each CREATE TABLE writes in its own transaction ([`super::Dialect::next_oid_after`])
    /// and an open loads ([`super::Dialect::load_next_oid`]): a reopen continues from it, never
    /// from the OIDs live tables hold (section 7).
    next_oid: AtomicU32,
    /// True once this database is confirmed PostgreSQL-dialect (fresh PostgreSQL instance or
    /// non-zero `pg_next_oid` loaded from page 1). False when `load_next_oid(0)` observed an
    /// unmarked file header, so unprefixed `__amphora_*` tables created by a SQLite owner are
    /// still rejected on native PostgreSQL open.
    has_pg_header_marker: AtomicBool,
}

impl Default for PostgresDialect {
    fn default() -> Self {
        Self::new()
    }
}

impl PostgresDialect {
    pub fn new() -> Self {
        Self {
            next_oid: AtomicU32::new(amphora_pg_catalog::FIRST_NORMAL_OBJECT_ID),
            has_pg_header_marker: AtomicBool::new(true),
        }
    }

    /// GetNewObjectId (varsup.c:578-596) in GetNewOidWithIndex's loop (REL_19_0
    /// src/backend/catalog/catalog.c:489-535): a counter below `FirstNormalObjectId`, after it
    /// wrapped through zero or in a database that never stored one, starts again there; it counts
    /// as an unsigned 32-bit `Oid`, so it wraps; and an OID a live table holds (`live`, sorted) is
    /// spent and passed over, as PostgreSQL passes over one `pg_class` holds.
    fn allocate_oid(&self, live: &[u32]) -> crate::Result<u32> {
        let normal = |next: u32| next.max(amphora_pg_catalog::FIRST_NORMAL_OBJECT_ID);
        // Of `live.len() + 1` OIDs in a row, one is free.
        for _ in 0..=live.len() {
            let oid = match self
                .next_oid
                .try_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                    Some(next_normal_oid(normal(next)))
                }) {
                Ok(previous) | Err(previous) => normal(previous),
            };
            if live.binary_search(&oid).is_err() {
                return Ok(oid);
            }
        }
        Err(LimboError::InternalError(
            "every OID the counter tried is held by a live table".to_string(),
        ))
    }

    /// The table definition in a stored definition.
    ///
    /// Tables without [`super::POSTGRES_TABLE_SQL_PREFIX`] are accepted only when they are
    /// engine-internal system tables (such as `__turso_internal_*` or `sqlite_*`) or Amphora
    /// owner-internal metadata tables (`__amphora_*`) on a PostgreSQL-marked database. User tables
    /// created by the SQLite dialect lack the prefix and are rejected so opening a SQLite
    /// database under [`PostgresDialect`] fails cleanly instead of mixing dialects in one file.
    fn definition<'a>(&self, sql: &'a str) -> crate::Result<&'a str> {
        match split_stored(sql)? {
            Some((_, definition)) => Ok(definition),
            None => {
                let ast::Stmt::CreateTable { tbl_name, .. } =
                    super::sqlite::parse_table_sql_ast(sql)?
                else {
                    return Err(LimboError::Corrupt(format!(
                        "persisted table SQL is not CREATE TABLE: {sql}"
                    )));
                };
                if crate::schema::is_system_table(tbl_name.name.as_str())
                    || (crate::schema::is_amphora_internal_table(tbl_name.name.as_str())
                        && self.has_pg_header_marker.load(Ordering::Relaxed))
                {
                    Ok(sql)
                } else {
                    Err(LimboError::Corrupt(format!(
                        "malformed database schema: table was created by the sqlite frontend: {sql}"
                    )))
                }
            }
        }
    }
}

/// A stored definition's `pg_class` OID and the table definition after it, or `None` for text
/// without the PostgreSQL prefix (the engine's own tables, which have no OID).
pub(crate) fn split_stored(sql: &str) -> crate::Result<Option<(u32, &str)>> {
    let Some(rest) = sql.strip_prefix(super::POSTGRES_TABLE_SQL_PREFIX) else {
        return Ok(None);
    };
    let corrupt = || {
        LimboError::Corrupt(format!(
            "malformed database schema: no pg_class OID in {sql}"
        ))
    };
    let (oid, definition) = rest
        .strip_prefix(OID_OPEN)
        .and_then(|r| r.split_once(OID_CLOSE))
        .ok_or_else(corrupt)?;
    let oid = oid.parse::<u32>().map_err(|_| corrupt())?;
    Ok(Some((oid, definition)))
}

/// The `pg_class` OIDs of the tables `conn`'s transaction sees in `database_id`, sorted: read from
/// that database's `sqlite_schema` (which `pg_class` is served from, [`crate::pg_catalog_vtab`]),
/// as GetNewOidWithIndex reads `pg_class`. CREATE TABLE asks while it is translated, before any
/// statement of its own runs, so the read is a statement of its own that ends its transaction
/// unless the caller's is open (an internal statement would leave it open, as it runs inside
/// another's).
fn live_oids(conn: &Arc<Connection>, database_id: usize) -> crate::Result<Vec<u32>> {
    let query = match database_id {
        crate::MAIN_DB_ID => "SELECT sql FROM main.sqlite_schema WHERE type = 'table'".to_string(),
        crate::TEMP_DB_ID => {
            if !conn.has_temp_database() {
                return Ok(Vec::new());
            }
            "SELECT sql FROM temp.sqlite_schema WHERE type = 'table'".to_string()
        }
        other => {
            let Some(name) = conn.get_database_name_by_index(other) else {
                return Ok(Vec::new());
            };
            let escaped = name.replace('"', "\"\"");
            format!("SELECT sql FROM \"{escaped}\".sqlite_schema WHERE type = 'table'")
        }
    };
    let mut stmt = conn.prepare(&query)?;
    let mut oids = Vec::new();
    for row in stmt.run_collect_rows()? {
        if let Some(Value::Text(sql)) = row.first() {
            if let Some((oid, _)) = split_stored(sql.as_str())? {
                oids.push(oid);
            }
        }
    }
    oids.sort_unstable();
    Ok(oids)
}

/// The stored definition of a table with `oid`.
fn stored(oid: u32, definition: &str) -> String {
    format!(
        "{}{OID_OPEN}{oid}{OID_CLOSE}{definition}",
        super::POSTGRES_TABLE_SQL_PREFIX
    )
}

/// A table's definition in the engine's canonical text, its columns' types checked to be
/// PostgreSQL 19 types as `pg_attribute` will record them.
fn definition_sql(
    tbl_name: &ast::QualifiedName,
    body: &ast::CreateTableBody,
) -> crate::Result<String> {
    let ast::CreateTableBody::ColumnsAndConstraints { columns, .. } = body else {
        return Err(LimboError::SqlError(
            "CREATE TABLE AS SELECT is not supported".to_string(),
        ));
    };
    for column in columns {
        let (name, modifiers) = match &column.col_type {
            Some(ast::Type {
                name,
                size,
                array_dimensions,
            }) => {
                if *array_dimensions > 0 {
                    return Err(LimboError::SqlError(format!(
                        "column \"{}\": array columns are not supported yet",
                        column.col_name.as_str()
                    )));
                }
                let modifiers = match size {
                    None => Vec::new(),
                    Some(ast::TypeSize::MaxSize(n)) => vec![type_modifier(n)?],
                    Some(ast::TypeSize::TypeSize(p, s)) => {
                        vec![type_modifier(p)?, type_modifier(s)?]
                    }
                };
                (name.as_str(), modifiers)
            }
            None => {
                return Err(LimboError::SqlError(format!(
                    "column \"{}\" has no type",
                    column.col_name.as_str()
                )))
            }
        };
        amphora_pg_catalog::column_type(name, &modifiers)
            .map_err(|e| LimboError::SqlError(e.to_string()))?;
    }
    Ok(format!(
        "CREATE TABLE {} {}",
        tbl_name.name.as_ident(),
        body
    ))
}

/// A type modifier: an integer constant, as `ArrayGetIntegerTypmods` takes it.
pub(crate) fn type_modifier(expr: &ast::Expr) -> crate::Result<i64> {
    let invalid =
        || LimboError::SqlError("type modifiers must be simple constants or identifiers".into());
    match expr {
        ast::Expr::Literal(ast::Literal::Numeric(n)) => n.parse::<i64>().map_err(|_| invalid()),
        ast::Expr::Unary(ast::UnaryOperator::Negative, inner) => match inner.as_ref() {
            ast::Expr::Literal(ast::Literal::Numeric(n)) => n
                .parse::<i64>()
                .ok()
                .and_then(i64::checked_neg)
                .ok_or_else(invalid),
            _ => Err(invalid()),
        },
        _ => Err(invalid()),
    }
}

impl super::Dialect for PostgresDialect {
    fn name(&self) -> &'static str {
        "postgres"
    }

    fn parse(&self, sql: &str) -> crate::Result<(Option<ast::Cmd>, usize)> {
        super::sqlite::parse(sql)
    }

    fn parse_table_sql(&self, sql: &str, root_page: i64) -> crate::Result<BTreeTable> {
        BTreeTable::from_sql(self.definition(sql)?, root_page)
    }

    fn parse_table_sql_ast(&self, sql: &str) -> crate::Result<ast::Stmt> {
        super::sqlite::parse_table_sql_ast(self.definition(sql)?)
    }

    fn table_sql_for_replay(&self, sql: &str) -> crate::Result<String> {
        if split_stored(sql)?.is_some() {
            return Err(LimboError::SqlError(
                "VACUUM of a PostgreSQL table is not supported yet: replaying its definition would give it a new pg_class OID".to_string(),
            ));
        }
        let definition = self.definition(sql)?;
        super::sqlite::table_sql_for_replay(definition)
    }

    /// A PostgreSQL table's OID is chosen against the tables a connection's transaction holds
    /// ([`super::Dialect::format_created_table_sql`]); a rewrite keeps the one it has
    /// ([`super::Dialect::format_rewritten_table_sql`]).
    fn format_table_sql(
        &self,
        _input: &str,
        _tbl_name: &ast::QualifiedName,
        _body: &ast::CreateTableBody,
    ) -> crate::Result<String> {
        Err(LimboError::InternalError(
            "a PostgreSQL table's definition is made with its connection".to_string(),
        ))
    }

    fn check_create_table_as_select(&self) -> crate::Result<()> {
        Err(LimboError::SqlError(
            "CREATE TABLE AS SELECT is not supported".to_string(),
        ))
    }

    fn format_created_table_sql(
        &self,
        conn: &Arc<Connection>,
        database_id: usize,
        _input: &str,
        tbl_name: &ast::QualifiedName,
        body: &ast::CreateTableBody,
    ) -> crate::Result<String> {
        let definition = definition_sql(tbl_name, body)?;
        // Tables the engine or Amphora owner makes for itself under reserved names (`sqlite_*`,
        // `__turso_internal_*`, `__amphora_*`) are not PostgreSQL public relations: they have no
        // row in `pg_class` and spend no OID, so the first application table takes
        // `FirstNormalObjectId` (16384), as PostgreSQL gives it (REL_19_0
        // src/backend/access/transam/varsup.c:578-605, src/backend/catalog/heap.c:1578).
        if crate::schema::is_system_table(tbl_name.name.as_str())
            || crate::schema::is_amphora_internal_table(tbl_name.name.as_str())
        {
            self.has_pg_header_marker.store(true, Ordering::Relaxed);
            return Ok(definition);
        }
        self.has_pg_header_marker.store(true, Ordering::Relaxed);
        Ok(stored(
            self.allocate_oid(&live_oids(conn, database_id)?)?,
            &definition,
        ))
    }

    fn fresh_instance(&self) -> Option<Arc<dyn super::Dialect>> {
        Some(Arc::new(PostgresDialect::new()))
    }

    fn allows_ungrouped_columns(&self) -> bool {
        false
    }

    fn load_next_oid(&self, stored: u32) {
        self.next_oid.store(stored, Ordering::Relaxed);
        self.has_pg_header_marker
            .store(stored != 0, Ordering::Relaxed);
    }

    fn advance_next_oid(&self, stored: u32) {
        if stored == 0 {
            return;
        }
        self.has_pg_header_marker.store(true, Ordering::Relaxed);
        let floor = stored.max(amphora_pg_catalog::FIRST_NORMAL_OBJECT_ID);
        let _ = self
            .next_oid
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |cur| {
                let normalized = cur.max(amphora_pg_catalog::FIRST_NORMAL_OBJECT_ID);
                oid_is_ahead(floor, normalized).then_some(floor)
            });
    }

    fn current_next_oid(&self) -> Option<u32> {
        Some(
            self.next_oid
                .load(Ordering::Relaxed)
                .max(amphora_pg_catalog::FIRST_NORMAL_OBJECT_ID),
        )
    }

    /// The OID after the table's, as `GetNewObjectId` leaves `nextOid`: past every OID a committed
    /// table took, so a reopen gives none of them again, dropped ones included. Wraps from
    /// `u32::MAX` to `FIRST_NORMAL_OBJECT_ID` so page 1 never stores `0`.
    fn next_oid_after(&self, stored_sql: &str) -> crate::Result<Option<u32>> {
        Ok(split_stored(stored_sql)?.map(|(oid, _)| next_normal_oid(oid)))
    }

    fn format_rewritten_table_sql(
        &self,
        stored_sql: &str,
        stmt: &ast::Stmt,
    ) -> crate::Result<String> {
        let ast::Stmt::CreateTable { tbl_name, body, .. } = stmt else {
            return Err(LimboError::InternalError(
                "format_rewritten_table_sql requires CREATE TABLE".to_string(),
            ));
        };
        let definition = definition_sql(tbl_name, body)?;
        Ok(match split_stored(stored_sql)? {
            Some((oid, _)) => stored(oid, &definition),
            None => definition,
        })
    }

    fn format_altered_table_sql(&self, _canonical: String) -> crate::Result<String> {
        Err(LimboError::SqlError(
            "ALTER TABLE ADD COLUMN and DROP COLUMN on a PostgreSQL table are not supported yet: the table's pg_class OID would not survive them".to_string(),
        ))
    }

    fn register_catalog(
        &self,
        schema: &mut Schema,
        _enable_custom_types: bool,
    ) -> crate::Result<()> {
        for catalog in PgCatalog::ALL {
            schema.register_internal_vtab(PgCatalogTable::new(catalog))?;
        }
        Ok(())
    }

    /// The engine's built-in functions, which the statements the engine generates call; which of
    /// them a PostgreSQL statement may name is analysis's (section 11's unit 8).
    fn resolve_function(&self, name: &str, arg_count: usize) -> crate::Result<Option<Func>> {
        super::sqlite::resolve_builtin_function(name, arg_count)
    }
}

#[cfg(test)]
mod tests {
    use super::PostgresDialect;
    use crate::dialect::Dialect;
    use crate::pg_catalog_vtab::value;
    use crate::sync::Arc;
    use crate::{ConnectOpts, Connection, Database, Numeric, PlatformIO, Value, IO};
    use amphora_pg_catalog::data::{PG_ATTRIBUTE, PG_CLASS, PG_NAMESPACE, PG_TYPE};
    use amphora_pg_catalog::Catalog;

    fn open(path: &str) -> (Arc<Database>, Arc<Connection>) {
        open_with(path, Arc::new(PostgresDialect::new()))
    }

    fn open_with(path: &str, dialect: Arc<PostgresDialect>) -> (Arc<Database>, Arc<Connection>) {
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let db = Database::open_file(io, path, dialect).unwrap();
        let conn = db.connect(ConnectOpts::new()).unwrap();
        (db, conn)
    }

    /// The `pg_class` OID of the table named `name`.
    fn oid(conn: &Arc<Connection>, name: &str) -> i64 {
        int(field(&PG_CLASS, &class_row(conn, name), "oid"))
    }

    fn query(conn: &Arc<Connection>, sql: &str) -> Vec<Vec<Value>> {
        conn.prepare(sql).unwrap().run_collect_rows().unwrap()
    }

    fn field<'a>(catalog: &Catalog, row: &'a [Value], name: &str) -> &'a Value {
        &row[catalog.column(name).unwrap()]
    }

    fn int(v: &Value) -> i64 {
        match v {
            Value::Numeric(Numeric::Integer(i)) => *i,
            other => panic!("not an integer: {other:?}"),
        }
    }

    fn text(v: &Value) -> String {
        match v {
            Value::Text(t) => t.as_str().to_string(),
            other => panic!("not text: {other:?}"),
        }
    }

    /// The `pg_class` row of the table named `name`.
    fn class_row(conn: &Arc<Connection>, name: &str) -> Vec<Value> {
        let mut rows = query(
            conn,
            &format!("SELECT * FROM pg_class WHERE relname = '{name}'"),
        );
        assert_eq!(rows.len(), 1, "one pg_class row for {name}");
        rows.remove(0)
    }

    /// T-PG.7.3: a database of the PostgreSQL dialect serves pg_namespace, pg_type, pg_class and
    /// pg_attribute, each with exactly PostgreSQL 19's built-in rows while it has no tables.
    #[test]
    fn t_pg_7_3_each_catalog_serves_19s_built_in_rows() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("built-in.db");
        let (_db, conn) = open(path.to_str().unwrap());
        for (catalog, count) in [
            (&PG_NAMESPACE, 3),
            (&PG_TYPE, 197),
            (&PG_CLASS, 4),
            (&PG_ATTRIBUTE, 145),
        ] {
            let got = query(&conn, &format!("SELECT * FROM {}", catalog.name));
            let expected: Vec<Vec<Value>> = catalog
                .rows
                .iter()
                .map(|row| row.iter().map(value).collect())
                .collect();
            assert_eq!(
                got.len(),
                count,
                "{} has 19's {count} built-in rows",
                catalog.name
            );
            assert_eq!(
                got, expected,
                "{} serves 19's rows as generated",
                catalog.name
            );
        }
    }

    /// T-PG.7.4: a table created through the engine shows in pg_class as an ordinary table of
    /// `public`, and in pg_attribute with its columns, their types and typmods as PostgreSQL 19
    /// records them, after the six system columns.
    #[test]
    fn t_pg_7_4_a_created_table_shows_in_pg_class_and_pg_attribute() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("created.db");
        let (_db, conn) = open(path.to_str().unwrap());
        conn.execute(
            "CREATE TABLE accounts (aid integer PRIMARY KEY, bid int4, abalance bigint NOT NULL, filler character(84), note varchar(10), amount numeric(12, 2), at timestamptz)",
        )
        .unwrap();

        let class = class_row(&conn, "accounts");
        let oid = int(field(&PG_CLASS, &class, "oid"));
        assert_eq!(oid, 16384, "the first table takes FirstNormalObjectId");
        assert_eq!(
            int(field(&PG_CLASS, &class, "relnamespace")),
            2200,
            "in public"
        );
        assert_eq!(text(field(&PG_CLASS, &class, "relkind")), "r");
        assert_eq!(text(field(&PG_CLASS, &class, "relpersistence")), "p");
        assert_eq!(int(field(&PG_CLASS, &class, "relnatts")), 7);
        assert_eq!(
            int(field(&PG_CLASS, &class, "relhasindex")),
            1,
            "a primary key is an index"
        );
        assert_eq!(int(field(&PG_CLASS, &class, "relam")), 2, "heap");

        let columns = query(
            &conn,
            &format!("SELECT attname, atttypid, atttypmod, attnotnull, attlen FROM pg_attribute WHERE attrelid = {oid} AND attnum > 0 ORDER BY attnum"),
        );
        let got: Vec<(String, i64, i64, i64, i64)> = columns
            .iter()
            .map(|r| (text(&r[0]), int(&r[1]), int(&r[2]), int(&r[3]), int(&r[4])))
            .collect();
        let numeric_12_2 = ((12 << 16) | 2) + 4;
        assert_eq!(
            got,
            vec![
                ("aid".to_string(), 23, -1, 1, 4),
                ("bid".to_string(), 23, -1, 0, 4),
                ("abalance".to_string(), 20, -1, 1, 8),
                ("filler".to_string(), 1042, 84 + 4, 0, -1),
                ("note".to_string(), 1043, 10 + 4, 0, -1),
                ("amount".to_string(), 1700, numeric_12_2, 0, -1),
                ("at".to_string(), 1184, -1, 0, 8),
            ]
        );
        let system = query(
            &conn,
            &format!("SELECT attname FROM pg_attribute WHERE attrelid = {oid} AND attnum < 0 ORDER BY attnum DESC"),
        );
        let system: Vec<String> = system.iter().map(|r| text(&r[0])).collect();
        assert_eq!(system, ["ctid", "xmin", "cmin", "xmax", "cmax", "tableoid"]);
    }

    /// T-PG.7.5: a table's OID is the same after a rename and after the database is reopened, and
    /// a table created after the reopen takes a new one; ADD COLUMN, which would lose it, is
    /// refused with a typed error.
    #[test]
    fn t_pg_7_5_a_tables_oid_survives_a_rename_and_a_reopen() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("reopen.db");
        let path = path.to_str().unwrap();
        let (first, second) = {
            let (_db, conn) = open(path);
            conn.execute("CREATE TABLE first (a int4)").unwrap();
            conn.execute("CREATE TABLE second (b text)").unwrap();
            let first = int(field(&PG_CLASS, &class_row(&conn, "first"), "oid"));
            let second = int(field(&PG_CLASS, &class_row(&conn, "second"), "oid"));
            conn.execute("ALTER TABLE second RENAME TO renamed")
                .unwrap();
            assert_eq!(
                int(field(&PG_CLASS, &class_row(&conn, "renamed"), "oid")),
                second
            );
            let refused = conn
                .execute("ALTER TABLE first ADD COLUMN c int4")
                .unwrap_err();
            assert!(
                refused.to_string().contains("not supported yet"),
                "{refused}"
            );
            (first, second)
        };
        assert_ne!(first, second);

        let (_db, conn) = open(path);
        assert_eq!(
            int(field(&PG_CLASS, &class_row(&conn, "first"), "oid")),
            first
        );
        assert_eq!(
            int(field(&PG_CLASS, &class_row(&conn, "renamed"), "oid")),
            second
        );
        conn.execute("CREATE TABLE third (c int8)").unwrap();
        let third = int(field(&PG_CLASS, &class_row(&conn, "third"), "oid"));
        assert!(
            third > first.max(second),
            "{third} is past every OID the reopened database holds"
        );
    }

    /// T-PG.7.6: the OID counter is durable (section 7): a table created after a dropped table
    /// and a reopen takes the OID after the dropped one's, never the dropped one's, as PostgreSQL
    /// keeps `nextOid` across restarts (REL_19_0 src/backend/access/transam/xlog.c:7280).
    #[test]
    fn t_pg_7_6_a_dropped_tables_oid_is_not_given_again_after_a_reopen() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("dropped.db");
        let path = path.to_str().unwrap();
        let dropped = {
            let (_db, conn) = open(path);
            conn.execute("CREATE TABLE gone (a int4)").unwrap();
            let dropped = oid(&conn, "gone");
            conn.execute("DROP TABLE gone").unwrap();
            dropped
        };

        let (_db, conn) = open(path);
        conn.execute("CREATE TABLE next (b int4)").unwrap();
        let next = oid(&conn, "next");
        assert_ne!(next, dropped, "the dropped table's OID is not given again");
        assert_eq!(
            next,
            dropped + 1,
            "the counter continues from its stored value"
        );
    }

    /// T-PG.7.7: two CREATE TABLEs in one transaction take distinct OIDs, and the transaction
    /// stores the counter past both: after the later table is dropped and the database reopened,
    /// a new table takes neither.
    #[test]
    fn t_pg_7_7_two_creates_in_one_transaction_take_distinct_oids_stored_past_both() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("one-transaction.db");
        let path = path.to_str().unwrap();
        let (first, second) = {
            let (_db, conn) = open(path);
            conn.execute("BEGIN").unwrap();
            conn.execute("CREATE TABLE first (a int4)").unwrap();
            conn.execute("CREATE TABLE second (b int4)").unwrap();
            conn.execute("COMMIT").unwrap();
            let first = oid(&conn, "first");
            let second = oid(&conn, "second");
            assert_ne!(first, second, "one transaction's tables take distinct OIDs");
            conn.execute("DROP TABLE second").unwrap();
            (first, second)
        };

        let (_db, conn) = open(path);
        conn.execute("CREATE TABLE third (c int4)").unwrap();
        let third = oid(&conn, "third");
        assert!(
            third != first && third != second,
            "{third} is neither {first} nor {second}"
        );
        assert_eq!(third, first.max(second) + 1);
    }

    /// T-PG.7.8: the counter wraps as PostgreSQL's (GetNewObjectId, varsup.c:578-596, in
    /// GetNewOidWithIndex's loop, catalog.c:489-535): past 4294967295 it starts again at
    /// FirstNormalObjectId, never below it, and passes over an OID a live table holds; the counter
    /// stored after the wrap is the one a reopen continues from.
    #[test]
    fn t_pg_7_8_the_oid_counter_wraps_past_reserved_and_live_oids() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("wrap.db");
        let path = path.to_str().unwrap();
        {
            let dialect = Arc::new(PostgresDialect::new());
            let (_db, conn) = open_with(path, dialect.clone());
            conn.execute("CREATE TABLE low (a int4)").unwrap();
            assert_eq!(oid(&conn, "low"), 16384);
            // As `pg_resetwal -o 4294967294` sets PostgreSQL's next OID.
            dialect.load_next_oid(u32::MAX - 1);
            conn.execute("CREATE TABLE top (a int4)").unwrap();
            conn.execute("CREATE TABLE last (a int4)").unwrap();
            conn.execute("CREATE TABLE wrapped (a int4)").unwrap();
            assert_eq!(oid(&conn, "top"), 4294967294);
            assert_eq!(oid(&conn, "last"), 4294967295, "the largest OID is given");
            assert_eq!(
                oid(&conn, "wrapped"),
                16385,
                "past the wrap: not 0, nor below FirstNormalObjectId, nor low's 16384"
            );
        }

        let (_db, conn) = open(path);
        conn.execute("CREATE TABLE after (a int4)").unwrap();
        assert_eq!(
            oid(&conn, "after"),
            16386,
            "a reopen continues after the wrap"
        );
    }

    /// T-PG.7.9: a rolled-back CREATE TABLE spends its OID: the counter is outside the transaction,
    /// as PostgreSQL's `nextOid` is (varsup.c:554-613), so the next table takes the OID after it.
    #[test]
    fn t_pg_7_9_a_rolled_back_create_table_spends_its_oid() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("rolled-back.db");
        let (_db, conn) = open(path.to_str().unwrap());
        conn.execute("BEGIN").unwrap();
        conn.execute("CREATE TABLE undone (a int4)").unwrap();
        let undone = oid(&conn, "undone");
        conn.execute("ROLLBACK").unwrap();
        conn.execute("CREATE TABLE kept (a int4)").unwrap();
        assert_eq!(oid(&conn, "kept"), undone + 1);
    }

    /// T-PG.7.10: under MVCC the counter reaches the page header through a checkpoint, and comes
    /// back from the page and from the log's recovery: a table created after a dropped one and a
    /// reopen takes the OID after the dropped one's, each way. The engine's own MVCC metadata
    /// table, made as `PRAGMA journal_mode = mvcc` switches, is no PostgreSQL relation: it takes no
    /// OID, so the first table takes FirstNormalObjectId as PostgreSQL's does, and `pg_class`
    /// doesn't show it.
    #[test]
    fn t_pg_7_10_the_counter_survives_mvcc_checkpoint_and_recovery() {
        if crate::storage::versioned::switch::pager_only(
            "this test exercises the pager's MVCC store, its log and its checkpoint",
        ) {
            return;
        }
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("mvcc.db");
        let path = path.to_str().unwrap();
        {
            let (_db, conn) = open(path);
            conn.execute("PRAGMA journal_mode = mvcc").unwrap();
            let internal = query(
                &conn,
                "SELECT relname FROM pg_class WHERE relname = '__turso_internal_mvcc_meta'",
            );
            assert!(internal.is_empty(), "pg_class shows the engine's own table");
            conn.execute("CREATE TABLE kept (a int4)").unwrap();
            conn.execute("CREATE TABLE gone (a int4)").unwrap();
            assert_eq!(oid(&conn, "kept"), 16384, "the first table's OID");
            assert_eq!(oid(&conn, "gone"), 16385);
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
            conn.execute("DROP TABLE gone").unwrap();
            conn.execute("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
            conn.close().unwrap();
        }
        {
            let (_db, conn) = open(path);
            conn.execute("CREATE TABLE checkpointed (a int4)").unwrap();
            assert_eq!(
                oid(&conn, "checkpointed"),
                16386,
                "from the page: past gone's 16385"
            );
            conn.execute("DROP TABLE checkpointed").unwrap();
            conn.close().unwrap();
        }
        let (_db, conn) = open(path);
        conn.execute("CREATE TABLE recovered (a int4)").unwrap();
        assert_eq!(oid(&conn, "kept"), 16384);
        assert_eq!(
            oid(&conn, "recovered"),
            16387,
            "gone took 16385 and checkpointed 16386, so neither comes back"
        );
    }

    /// Review regression: opening an existing cold SQLite database with `PostgresDialect` must
    /// reject its SQLite user tables instead of silently accepting them and allowing subsequent
    /// PostgreSQL DDL to corrupt reopening under `SqliteDialect`.
    #[test]
    fn review_postgres_opener_rejects_cold_sqlite_database() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("sqlite-user.db");
        let path_str = path.to_str().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());

        {
            let db =
                Database::open_file(io.clone(), path_str, Arc::new(crate::SqliteDialect)).unwrap();
            let conn = db.connect(ConnectOpts::new()).unwrap();
            conn.execute("CREATE TABLE items(id INTEGER PRIMARY KEY, label TEXT)")
                .unwrap();
            conn.execute("INSERT INTO items VALUES (1, 'hello')")
                .unwrap();
            conn.close().unwrap();
        }

        let err = match Database::open_file(io.clone(), path_str, Arc::new(PostgresDialect::new()))
        {
            Ok(_) => panic!("PostgresDialect must reject a cold SQLite database with user tables"),
            Err(err) => err,
        };
        assert!(
            err.to_string()
                .contains("table was created by the sqlite frontend"),
            "unexpected error opening SQLite database with PostgresDialect: {err}"
        );

        // The original SQLite database must still reopen cleanly under SqliteDialect.
        let reopened = Database::open_file(io, path_str, Arc::new(crate::SqliteDialect)).unwrap();
        let conn = reopened.connect(ConnectOpts::new()).unwrap();
        let rows = query(&conn, "SELECT id, label FROM items");
        assert_eq!(
            rows,
            vec![vec![
                Value::from_i64(1),
                Value::from_text("hello".to_string())
            ]]
        );
    }

    /// Review Round 7 Spec P2 regression: attaching another PostgreSQL database must give the
    /// attached database its own `PostgresDialect` instance so opening a lower-OID attached file
    /// never rewinds `main`'s live or durable `pg_next_oid` counter.
    #[test]
    fn review_p2_attach_does_not_rewind_main_postgres_next_oid() {
        let dir = tempfile::tempdir().unwrap();
        let main_path = dir.path().join("main.db");
        let other_path = dir.path().join("other.db");
        let io: Arc<dyn crate::IO> = Arc::new(crate::PlatformIO::new().unwrap());

        // Seed `main.db` by creating and dropping a table (advancing pg_next_oid to 16385).
        // Attach a fresh PostgreSQL file `other.db` (pg_next_oid = 16384), then create a main view
        // (persisting main's current_next_oid) and an attached view (persisting other's marker).
        {
            let db = Database::open_file_with_flags(
                io.clone(),
                main_path.to_str().unwrap(),
                crate::OpenFlags::default(),
                crate::DatabaseOpts::new().with_attach(true),
                None,
                Arc::new(PostgresDialect::new()),
            )
            .unwrap();
            let conn = db.connect(ConnectOpts::new()).unwrap();
            conn.execute("CREATE TABLE gone(id int4)").unwrap();
            conn.execute("DROP TABLE gone").unwrap();
            conn.execute(format!(
                "ATTACH '{}' AS other",
                other_path.to_str().unwrap()
            ))
            .unwrap();
            conn.execute("CREATE VIEW main_v AS SELECT 1 AS v").unwrap();
            conn.execute("CREATE VIEW other.att_v AS SELECT 73 AS val")
                .unwrap();
            conn.execute("DETACH other").unwrap();
            conn.close().unwrap();
        }

        // Reopen `main.db` cold: creating a new table must receive OID 16385, never dropped 16384.
        {
            let reopened = Database::open_file(
                io.clone(),
                main_path.to_str().unwrap(),
                Arc::new(PostgresDialect::new()),
            )
            .unwrap();
            let conn = reopened.connect(ConnectOpts::new()).unwrap();
            conn.execute("CREATE TABLE after_view(id int4)").unwrap();
            let rows = query(
                &conn,
                "SELECT oid FROM pg_class WHERE relname = 'after_view'",
            );
            assert_eq!(
                rows,
                vec![vec![Value::from_i64(16385)]],
                "ATTACH must not rewind main's durable pg_next_oid back to dropped OID 16384"
            );
            conn.close().unwrap();
        }

        // `other.db` (view-only created through ATTACH) must carry its persisted pg_next_oid
        // marker (including in WAL before checkpoint) so SqliteDialect rejects it and
        // PostgresDialect opens it cleanly.
        let err = match Database::open_file(
            io.clone(),
            other_path.to_str().unwrap(),
            Arc::new(crate::SqliteDialect),
        ) {
            Ok(_) => panic!("SqliteDialect must reject attached view-only PostgreSQL database"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("postgres"),
            "expected PostgreSQL dialect error for attached view-only file, got: {err}"
        );
        let other_pg = Database::open_file(
            io,
            other_path.to_str().unwrap(),
            Arc::new(PostgresDialect::new()),
        )
        .unwrap();
        let other_conn = other_pg.connect(ConnectOpts::new()).unwrap();
        assert_eq!(
            query(&other_conn, "SELECT val FROM att_v"),
            vec![vec![Value::from_i64(73)]]
        );
        other_conn
            .execute("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        other_conn.close().unwrap();
        let mut page1 = [0u8; 4096];
        let mut f = std::fs::File::open(&other_path).unwrap();
        std::io::Read::read_exact(&mut f, &mut page1).unwrap();
        assert_eq!(
            crate::header_pg_next_oid(&page1),
            Some(16384),
            "CREATE VIEW on attached PostgreSQL database must persist pg_next_oid on page 1"
        );
    }

    /// Review Round 8 P2 regression: distinct PostgreSQL temporary tables must receive distinct
    /// OIDs (both under `PRAGMA temp_store = MEMORY` and `FILE`) without spending `main`'s OID
    /// allocator on the first temporary table.
    #[test]
    fn review_p2_postgres_temp_tables_receive_distinct_oids_without_touching_main() {
        for temp_store in ["MEMORY", "FILE"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(format!("temp_oid_{temp_store}.db"));
            let (_db, conn) = open(path.to_str().unwrap());
            conn.execute(format!("PRAGMA temp_store = {temp_store}"))
                .unwrap();
            conn.execute("CREATE TEMP TABLE t1 (id int4)").unwrap();
            conn.execute("INSERT INTO t1 VALUES (10)").unwrap();
            conn.execute("CREATE TEMP TABLE t2 (id int4)").unwrap();
            conn.execute("INSERT INTO t2 VALUES (20)").unwrap();

            let rows = query(
                &conn,
                "SELECT name, sql FROM temp.sqlite_schema WHERE type = 'table' ORDER BY name",
            );
            assert_eq!(
                rows.len(),
                2,
                "temp_store={temp_store}: expected 2 temp tables"
            );
            let oid1 = super::split_stored(&text(&rows[0][1]))
                .unwrap()
                .expect("t1 must carry a PostgreSQL OID")
                .0;
            let oid2 = super::split_stored(&text(&rows[1][1]))
                .unwrap()
                .expect("t2 must carry a PostgreSQL OID")
                .0;
            assert_eq!(oid1, 16384, "temp_store={temp_store}: first temp table OID");
            assert_eq!(
                oid2, 16385,
                "temp_store={temp_store}: second temp table must get distinct OID 16385, not duplicate 16384"
            );
            assert_eq!(
                query(&conn, "SELECT id FROM t1"),
                vec![vec![Value::from_i64(10)]]
            );
            assert_eq!(
                query(&conn, "SELECT id FROM t2"),
                vec![vec![Value::from_i64(20)]]
            );

            // Main database's allocator must remain untouched at 16384.
            conn.execute("CREATE TABLE main_t (id int4)").unwrap();
            assert_eq!(
                oid(&conn, "main_t"),
                16384,
                "temp_store={temp_store}: creating temp tables must not consume main's first OID"
            );
        }
    }

    /// Review Round 8 P3 regression: unsupported PostgreSQL `CREATE TABLE AS SELECT` must return
    /// a clean `SqlError` (not `Corrupt("malformed database schema...")`), leaving the database
    /// healthy and the target table absent.
    #[test]
    fn review_p3_postgres_ctas_returns_sql_error_not_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pg_ctas.db");
        let (_db, conn) = open(path.to_str().unwrap());
        conn.execute("CREATE TABLE src (id int4)").unwrap();
        conn.execute("INSERT INTO src VALUES (42)").unwrap();

        let err = conn
            .execute("CREATE TABLE dst AS SELECT id FROM src")
            .unwrap_err();
        assert!(
            matches!(err, crate::LimboError::SqlError(ref msg) if msg.contains("CREATE TABLE AS SELECT is not supported")),
            "PostgreSQL CTAS must return SqlError('CREATE TABLE AS SELECT is not supported'), got: {err:?}"
        );
        let tables = query(
            &conn,
            "SELECT name FROM sqlite_schema WHERE type = 'table' AND name = 'dst'",
        );
        assert!(tables.is_empty(), "failed CTAS must not create table dst");
        assert_eq!(
            query(&conn, "SELECT id FROM src"),
            vec![vec![Value::from_i64(42)]]
        );
    }

    /// Review Round 9 Spec P1 regression:
    /// - SQLite grouped (sorter and index-ordered) and ungrouped `MIN`/`MAX` with bare columns
    ///   must return the bare column from the winning `MIN`/`MAX` row.
    /// - PostgreSQL must reject ungrouped non-aggregate columns in `SELECT`/`HAVING`/`ORDER BY`.
    #[test]
    fn review_r9_p1_grouped_min_max_bare_columns_on_sqlite_and_rejected_on_postgres() {
        let dir = tempfile::tempdir().unwrap();

        // SQLite: sorter GROUP BY, indexed ASC GROUP BY, indexed DESC GROUP BY, and ungrouped.
        let sq_path = dir.path().join("sqlite_minmax_bare.db");
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());
        let sq_db = Database::open_file(
            io.clone(),
            sq_path.to_str().unwrap(),
            Arc::new(crate::SqliteDialect),
        )
        .unwrap();
        let sq = sq_db.connect(ConnectOpts::new()).unwrap();
        sq.execute("CREATE TABLE t (g INTEGER, label TEXT, v INTEGER)")
            .unwrap();
        sq.execute(
            "INSERT INTO t VALUES (1, 'first', 1), (1, 'winner', 9), (2, 'low', 2), (2, 'high', 8)",
        )
        .unwrap();

        assert_eq!(
            query(&sq, "SELECT label, MAX(v) FROM t GROUP BY g ORDER BY g"),
            vec![
                vec![Value::from_text("winner"), Value::from_i64(9)],
                vec![Value::from_text("high"), Value::from_i64(8)],
            ]
        );
        assert_eq!(
            query(&sq, "SELECT label, MIN(v) FROM t GROUP BY g ORDER BY g"),
            vec![
                vec![Value::from_text("first"), Value::from_i64(1)],
                vec![Value::from_text("low"), Value::from_i64(2)],
            ]
        );
        assert_eq!(
            query(&sq, "SELECT label, MAX(v) FROM t"),
            vec![vec![Value::from_text("winner"), Value::from_i64(9)]]
        );
        assert_eq!(
            query(&sq, "SELECT label, MIN(v) FROM t"),
            vec![vec![Value::from_text("first"), Value::from_i64(1)]]
        );

        // With an index on (g, v DESC) so GroupByRowSource::MainLoop reads ('winner', 9) before ('first', 1).
        sq.execute("CREATE INDEX idx_g_v_desc ON t(g, v DESC)")
            .unwrap();
        assert_eq!(
            query(&sq, "SELECT label, MIN(v) FROM t GROUP BY g ORDER BY g"),
            vec![
                vec![Value::from_text("first"), Value::from_i64(1)],
                vec![Value::from_text("low"), Value::from_i64(2)],
            ]
        );
        assert_eq!(
            query(&sq, "SELECT label, MAX(v) FROM t GROUP BY g ORDER BY g"),
            vec![
                vec![Value::from_text("winner"), Value::from_i64(9)],
                vec![Value::from_text("high"), Value::from_i64(8)],
            ]
        );

        // PostgreSQL: ungrouped non-aggregate columns must be rejected.
        let pg_path = dir.path().join("pg_strict_groupby.db");
        let (_pg_db, pg) = open(pg_path.to_str().unwrap());
        pg.execute("CREATE TABLE t (g int4, label text, v int4)")
            .unwrap();
        pg.execute("INSERT INTO t VALUES (1, 'first', 1), (1, 'winner', 9)")
            .unwrap();
        for bad_sql in [
            "SELECT label, MAX(v) FROM t GROUP BY g",
            "SELECT label, MAX(v) FROM t",
            "SELECT g, MAX(v) FROM t GROUP BY g ORDER BY label",
            "SELECT g, MAX(v) FROM t GROUP BY g HAVING label = 'winner'",
            "SELECT COUNT(*) FROM t ORDER BY label",
        ] {
            let err = pg.prepare(bad_sql).unwrap_err();
            assert!(
                err.to_string().contains("GROUP BY"),
                "PostgreSQL must reject ungrouped column in `{bad_sql}`, got: {err:?}"
            );
        }
        // Valid grouped query and primary-key functionally dependent query still succeed.
        assert_eq!(
            query(&pg, "SELECT g, MAX(v) FROM t GROUP BY g"),
            vec![vec![Value::from_i64(1), Value::from_i64(9)]]
        );
        pg.execute("CREATE TABLE pk_t (id int4 PRIMARY KEY, label text, v int4)")
            .unwrap();
        pg.execute("INSERT INTO pk_t VALUES (1, 'ok', 5)").unwrap();
        assert_eq!(
            query(&pg, "SELECT id, label, MAX(v) FROM pk_t GROUP BY id"),
            vec![vec![
                Value::from_i64(1),
                Value::from_text("ok"),
                Value::from_i64(5)
            ]]
        );
    }

    /// Review Round 9 Spec P2 regression: `COUNT(expr)` must not treat arithmetic on NOT NULL
    /// operands as guaranteed non-NULL, because IEEE-754 overflow (`1e308 * 1e308 = +Inf`,
    /// `(+Inf) - (+Inf) = NaN`) evaluates to `NULL` and must return `0`.
    #[test]
    fn review_r9_p2_count_expression_nan_overflow_returns_zero_on_both_dialects() {
        let dir = tempfile::tempdir().unwrap();
        let io: Arc<dyn IO> = Arc::new(PlatformIO::new().unwrap());

        let sq_db = Database::open_file(
            io.clone(),
            dir.path().join("sq_nan.db").to_str().unwrap(),
            Arc::new(crate::SqliteDialect),
        )
        .unwrap();
        let sq = sq_db.connect(ConnectOpts::new()).unwrap();
        sq.execute("CREATE TABLE t (x REAL NOT NULL); INSERT INTO t VALUES (1e308)")
            .unwrap();

        let (_pg_db, pg) = open(dir.path().join("pg_nan.db").to_str().unwrap());
        pg.execute("CREATE TABLE t (x float8 NOT NULL); INSERT INTO t VALUES (1e308)")
            .unwrap();

        for (label, conn) in [("sqlite", &sq), ("postgres", &pg)] {
            assert_eq!(
                query(conn, "SELECT COUNT((x * x) - (x * x)) FROM t"),
                vec![vec![Value::from_i64(0)]],
                "{label}: COUNT((x*x)-(x*x)) on 1e308 must return 0"
            );
            assert_eq!(
                query(conn, "SELECT COUNT((x * x) - (x * x)), COUNT(*) FROM t"),
                vec![vec![Value::from_i64(0), Value::from_i64(1)]],
                "{label}: COUNT((x*x)-(x*x)), COUNT(*) must agree"
            );
            assert_eq!(
                query(conn, "SELECT COUNT((x * x) + (-(x * x))) FROM t"),
                vec![vec![Value::from_i64(0)]],
                "{label}: COUNT(+Inf + -Inf) must return 0"
            );
            assert_eq!(
                query(conn, "SELECT COUNT((x * x) * 0.0) FROM t"),
                vec![vec![Value::from_i64(0)]],
                "{label}: COUNT(+Inf * 0.0) must return 0"
            );
        }
    }

    /// Review Round 9 Spec P3 regression: `PostgresDialect::advance_next_oid` must advance across
    /// `u32::MAX` wraparound (e.g. `4294967294 -> 16386`), reject stale pre-wrap frontiers after
    /// wraparound, and `next_oid_after` at `u32::MAX` must wrap to `FIRST_NORMAL_OBJECT_ID`
    /// (`16384`) rather than persisting `0`.
    #[test]
    fn review_r9_p3_postgres_oid_advance_and_persistence_across_u32_wraparound() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pg_oid_wrap.db");
        let dialect = Arc::new(PostgresDialect::new());
        let (_db, conn) = open_with(path.to_str().unwrap(), dialect.clone());

        // Initialize near u32::MAX, then advance across wraparound to 16386.
        dialect.load_next_oid(u32::MAX - 1); // 4294967294
        dialect.advance_next_oid(16386);
        assert_eq!(dialect.current_next_oid(), Some(16386));

        // Stale pre-wrap frontier must NOT rewind the post-wrap counter.
        dialect.advance_next_oid(u32::MAX - 1);
        assert_eq!(dialect.current_next_oid(), Some(16386));

        conn.execute("CREATE TABLE wrapped (id int4)").unwrap();
        assert_eq!(
            oid(&conn, "wrapped"),
            16386,
            "table created after wraparound sync must receive OID 16386, not 4294967294"
        );

        // Creating a table at u32::MAX must persist next_oid = 16384 (never 0).
        dialect.load_next_oid(u32::MAX);
        conn.execute("CREATE TABLE at_max (id int4)").unwrap();
        assert_eq!(oid(&conn, "at_max"), i64::from(u32::MAX));
        assert_eq!(dialect.current_next_oid(), Some(16384));
        conn.checkpoint(crate::CheckpointMode::Truncate {
            upper_bound_inclusive: None,
        })
        .unwrap();
        drop(conn);
        drop(_db);
        let page1 = std::fs::read(&path).unwrap();
        assert_eq!(
            crate::header_pg_next_oid(&page1),
            Some(16384),
            "wrapping past u32::MAX must persist pg_next_oid=16384 on page 1, never 0"
        );
    }
}
