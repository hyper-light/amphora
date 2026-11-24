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
use crate::sync::atomic::{AtomicU32, Ordering};
use crate::LimboError;
use amphora_engine_parser::ast;

/// What follows the prefix in a stored definition: `/* pg_class.oid <oid> */ `.
const OID_OPEN: &str = "/* pg_class.oid ";
const OID_CLOSE: &str = " */ ";

/// The PostgreSQL dialect.
pub struct PostgresDialect {
    /// The OID the next table takes: PostgreSQL's OID counter, outside the caller's transaction
    /// (section 7), so a rolled-back CREATE TABLE spends its OID as PostgreSQL's does. It starts at
    /// `FirstNormalObjectId` and passes every OID a loaded definition holds, so a reopened database
    /// never gives a live table's OID to another.
    next_oid: AtomicU32,
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
        }
    }

    fn allocate_oid(&self) -> crate::Result<u32> {
        self.next_oid
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |oid| {
                oid.checked_add(1)
            })
            .map_err(|_| {
                LimboError::SqlError(
                    "no OID is left for a new table: OIDs don't wrap around yet".to_string(),
                )
            })
    }

    /// The table definition in a stored definition, noting its OID as taken.
    fn definition<'a>(&self, sql: &'a str) -> crate::Result<&'a str> {
        Ok(match split_stored(sql)? {
            Some((oid, definition)) => {
                if let Some(next) = oid.checked_add(1) {
                    self.next_oid.fetch_max(next, Ordering::Relaxed);
                }
                definition
            }
            None => sql,
        })
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
        super::sqlite::table_sql_for_replay(sql)
    }

    fn format_table_sql(
        &self,
        _input: &str,
        tbl_name: &ast::QualifiedName,
        body: &ast::CreateTableBody,
    ) -> crate::Result<String> {
        let definition = definition_sql(tbl_name, body)?;
        Ok(stored(self.allocate_oid()?, &definition))
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

    /// T-PG.7.8 (red run on main): past 4294967295 the counter starts again at
    /// FirstNormalObjectId and passes over an OID a live table holds.
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
            dialect
                .next_oid
                .store(u32::MAX - 1, crate::sync::atomic::Ordering::Relaxed);
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

    /// T-PG.7.10 (red run on main).
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
}
