//! INSTEAD OF triggers (lane C28; AC-T1.1.1, AC-T1.8.6): what one database and one draw of
//! `bench/sqlite_instead_of.py` can't hold: a schema read again when its file is opened, ALTER TABLE
//! beside a view's triggers, and where sqlite3 3.54.0 does not do what its documents say.

use amphora_engine::{
    ConnectOpts, Connection, Database, Numeric, OpenOptions, PlatformIO, SqliteDialect, StepResult,
    Value, IO,
};
use std::path::Path;
use std::sync::Arc;

struct Held {
    io: Arc<PlatformIO>,
    conn: Arc<Connection>,
}

fn open(path: &Path) -> Held {
    let io = Arc::new(PlatformIO::new().unwrap());
    let options = OpenOptions::new(Arc::new(SqliteDialect));
    let db = Database::open(io.clone(), path.to_str().unwrap(), options).unwrap();
    let conn = db.connect(ConnectOpts::new()).unwrap();
    Held { io, conn }
}

impl Held {
    /// The rows of `sql`, each row's values joined by commas, or its refusal.
    fn ask(&self, sql: &str) -> Result<Vec<String>, String> {
        let mut stmt = self.conn.prepare(sql).map_err(|error| error.to_string())?;
        let mut rows = Vec::new();
        loop {
            match stmt.step().map_err(|error| error.to_string())? {
                StepResult::Row => {
                    let values: Vec<String> = stmt
                        .row()
                        .unwrap()
                        .get_values()
                        .map(|value| match value {
                            Value::Null => "NULL".to_owned(),
                            Value::Numeric(Numeric::Float(real)) => format!("{}", f64::from(*real)),
                            value => value.to_string(),
                        })
                        .collect();
                    rows.push(values.join(","));
                }
                StepResult::Done => return Ok(rows),
                StepResult::IO | StepResult::Yield => {
                    self.io.step().map_err(|error| error.to_string())?
                }
                other => return Err(format!("step: {other:?}")),
            }
        }
    }

    fn run(&self, statements: &[&str]) {
        for sql in statements {
            self.ask(sql)
                .unwrap_or_else(|error| panic!("{sql}: {error}"));
        }
    }

    fn refuses(&self, sql: &str, words: &str) {
        let error = self.ask(sql).expect_err(sql);
        assert!(error.ends_with(words), "{sql}: {error}");
    }
}

const BASE: [&str; 5] = [
    "CREATE TABLE t(k INTEGER PRIMARY KEY, a INT, b TEXT)",
    "CREATE TABLE u(n INT, w TEXT)",
    "CREATE TABLE lg(x, y, z)",
    "INSERT INTO t VALUES (1, 10, 'x'), (2, 20, 'y'), (3, 30, 'z')",
    "INSERT INTO u VALUES (1, 'one'), (2, 'two')",
];

/// Do: make a view and its triggers, close the file and open it again. Expect: they are there, with
/// their text, and fire; dropped, they are gone from the file too, and so are those of a view dropped.
#[test]
fn a_file_opened_again_has_its_views_triggers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reopened.db");
    let held = open(&path);
    held.run(&BASE);
    held.run(&[
        "CREATE VIEW v AS SELECT k, a, b FROM t",
        "CREATE VIEW o AS SELECT k FROM v",
        "CREATE TRIGGER gu INSTEAD OF UPDATE OF a ON v FOR EACH ROW WHEN new.a > old.a BEGIN UPDATE t SET a = new.a WHERE k = old.k; INSERT INTO lg VALUES (old.k, old.a, new.a); END",
        "CREATE TRIGGER gi INSTEAD OF INSERT ON main.v BEGIN INSERT INTO t VALUES (new.k, new.a, new.b); END",
        "CREATE TRIGGER gd INSTEAD OF DELETE ON o BEGIN DELETE FROM t WHERE k = old.k; END",
    ]);
    let sql = "SELECT name || ':' || tbl_name || ':' || rootpage FROM sqlite_schema WHERE type = 'trigger' ORDER BY name";
    let stored = held.ask(sql).unwrap();
    assert_eq!(stored, ["gd:o:0", "gi:v:0", "gu:v:0"]);
    drop(held);

    let held = open(&path);
    assert_eq!(held.ask(sql).unwrap(), stored);
    held.run(&[
        "INSERT INTO v VALUES (4, 40, 'w')",
        "UPDATE v SET a = a + CASE k WHEN 2 THEN 5 ELSE -5 END",
        "DELETE FROM o WHERE k = 1",
    ]);
    assert_eq!(
        held.ask("SELECT k, a FROM t ORDER BY k").unwrap(),
        ["2,25", "3,30", "4,40"]
    );
    assert_eq!(held.ask("SELECT * FROM lg").unwrap(), ["2,20,25"]);
    assert_eq!(held.ask("PRAGMA integrity_check").unwrap(), ["ok"]);
    held.run(&["DROP TRIGGER gu", "DROP VIEW o"]);
    drop(held);

    let held = open(&path);
    assert_eq!(held.ask(sql).unwrap(), ["gi:v:0"]);
    held.refuses("UPDATE v SET a = 1", "cannot modify v because it is a view");
    held.run(&["INSERT INTO v VALUES (5, 50, 'v')"]);
    assert_eq!(held.ask("SELECT count(*) FROM t").unwrap(), ["4"]);
}

/// Do: ALTER TABLE while a view has a trigger. Expect: a table no trigger reads is altered, the
/// trigger's statements follow a table and a column renamed under them, and it fires after.
#[test]
fn alter_table_beside_a_views_trigger() {
    let dir = tempfile::tempdir().unwrap();
    let held = open(&dir.path().join("altered.db"));
    held.run(&BASE);
    held.run(&[
        "CREATE VIEW v AS SELECT a AS p, b AS q FROM t",
        "CREATE TRIGGER g INSTEAD OF INSERT ON v BEGIN INSERT INTO t(a, b) VALUES (new.p, new.q); END",
        "ALTER TABLE u RENAME COLUMN w TO ww",
        "ALTER TABLE u DROP COLUMN ww",
        "ALTER TABLE t RENAME COLUMN a TO aa",
        "ALTER TABLE t RENAME TO tt",
        "ALTER TABLE tt ADD COLUMN c",
        "INSERT INTO v VALUES (7, 'seven')",
    ]);
    assert_eq!(
        held.ask("SELECT k, aa, b FROM tt WHERE k > 3").unwrap(),
        ["4,7,seven"]
    );
}

/// lang_update.html and lang_delete.html: the target may have an alias, which its names are then by.
/// sqlite3 3.54.0 refuses one in WHERE ("no such column: q.k": it stores the view's rows under the
/// view's own name), and takes it everywhere else.
#[test]
fn an_alias_names_the_view_everywhere() {
    let dir = tempfile::tempdir().unwrap();
    let held = open(&dir.path().join("alias.db"));
    held.run(&BASE);
    held.run(&[
        "CREATE VIEW v AS SELECT k, a, b FROM t",
        "CREATE TRIGGER gu INSTEAD OF UPDATE ON v BEGIN INSERT INTO lg VALUES ('u', old.k, new.b); END",
        "CREATE TRIGGER gd INSTEAD OF DELETE ON v BEGIN INSERT INTO lg VALUES ('d', old.k, old.b); END",
        "UPDATE v AS q SET b = q.b || q.a WHERE q.k = 2",
        "DELETE FROM v AS q WHERE q.k > 2 AND EXISTS (SELECT 1 FROM t WHERE t.k = q.k)",
    ]);
    assert_eq!(held.ask("SELECT * FROM lg").unwrap(), ["u,2,y20", "d,3,z"]);
    held.ask("UPDATE v AS q SET b = 1 WHERE v.k = 2")
        .expect_err("the alias is the view's one name");
}

/// lang_createview.html: "You cannot DELETE, INSERT, or UPDATE a view", but by an INSTEAD OF trigger,
/// which lang_createtrigger.html has for each statement. sqlite3 3.54.0 takes a statement with
/// RETURNING where any trigger is on the view, returns its rows and does nothing.
#[test]
fn returning_does_not_stand_in_for_a_trigger() {
    let dir = tempfile::tempdir().unwrap();
    let held = open(&dir.path().join("returning.db"));
    held.run(&BASE);
    held.run(&[
        "CREATE VIEW v AS SELECT k, a, b FROM t",
        "CREATE TRIGGER gd INSTEAD OF DELETE ON v BEGIN INSERT INTO lg VALUES (old.k, 0, 0); END",
    ]);
    let refused = "cannot modify v because it is a view";
    held.refuses("INSERT INTO v VALUES (1, 2, 3) RETURNING k", refused);
    held.refuses("UPDATE v SET a = 1 RETURNING k", refused);
    assert_eq!(
        held.ask("DELETE FROM v RETURNING k").unwrap(),
        ["1", "2", "3"]
    );
}

/// lang_update.html: "Columns that do not appear in the list of assignments are unmodified when the
/// row is updated." sqlite3 3.54.0 has them NULL in old and new where an UPDATE FROM is of a view over
/// a compound.
#[test]
fn an_update_from_keeps_the_columns_it_does_not_set() {
    let dir = tempfile::tempdir().unwrap();
    let held = open(&dir.path().join("update_from.db"));
    held.run(&BASE);
    held.run(&[
        "CREATE VIEW v AS SELECT k, a, b FROM t UNION ALL SELECT n, n + 40, w FROM u ORDER BY 2",
        "CREATE VIEW g AS SELECT b AS k, count(*) AS a, max(k) AS b FROM t GROUP BY b",
        "CREATE TRIGGER gv INSTEAD OF UPDATE ON v BEGIN INSERT INTO lg VALUES (old.b, new.a, new.b); END",
        "CREATE TRIGGER gg INSTEAD OF UPDATE ON g BEGIN INSERT INTO lg VALUES (old.k, new.a, new.b); END",
        "UPDATE v SET a = w.n + 1 FROM u AS w WHERE w.n = v.k AND v.k = 2",
        "UPDATE g SET a = w.w FROM u AS w WHERE w.n = g.b",
    ]);
    let mut rows = held.ask("SELECT * FROM lg").unwrap();
    rows.sort();
    assert_eq!(rows, ["two,3,two", "x,one,1", "y,3,y", "y,two,2"]);
}
