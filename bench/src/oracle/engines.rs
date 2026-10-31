//! The two engines the oracles run on, behind one interface: Amphora through its engine crate (an
//! in-memory database each, as production opens one but with statement journals on, so a failed
//! statement leaves nothing behind as SQLite's don't) and the system's SQLite, the reference.

use std::sync::Arc;

/// A value as both engines give it back: SQLite's five storage classes.
#[derive(Clone, Debug)]
pub enum V {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

pub type Rows = Vec<Vec<V>>;

/// SQLite's answer held text that isn't well-formed UTF-8: under its invalid UTF policy (garbage in,
/// garbage out) no answer is promised, and Amphora's TEXT is always well-formed
/// (docs/design/semantics.md, decision 1), so there is nothing to compare.
pub const ILL_FORMED: &str = "SQLite answered text that isn't well-formed UTF-8";

impl V {
    fn rank(&self) -> u8 {
        match self {
            V::Null => 0,
            V::Int(_) | V::Real(_) => 1,
            V::Text(_) => 2,
            V::Blob(_) => 3,
        }
    }

    /// The same value, a REAL within a relative 1e-9 of the other (math functions' last digits
    /// differ between libms); classes must match, as `typeof` would show.
    pub fn same(&self, o: &V) -> bool {
        match (self, o) {
            (V::Null, V::Null) => true,
            (V::Int(a), V::Int(b)) => a == b,
            (V::Real(a), V::Real(b)) => a == b || (a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)),
            (V::Text(a), V::Text(b)) => a == b,
            (V::Blob(a), V::Blob(b)) => a == b,
            _ => false,
        }
    }

    /// As SQL writes it, for a finding's statements and messages.
    pub fn sql(&self) -> String {
        match self {
            V::Null => "NULL".into(),
            V::Int(i) => i.to_string(),
            V::Real(f) if f.is_finite() => {
                let s = format!("{f:?}");
                if s.contains(['e', 'E', '.']) { s } else { format!("{s}.0") }
            }
            V::Real(f) => if *f > 0.0 { "1e999".into() } else if *f < 0.0 { "-1e999".into() } else { "NULL".into() },
            V::Text(s) => format!("'{}'", s.replace('\'', "''")),
            V::Blob(b) => format!("x'{}'", b.iter().map(|x| format!("{x:02x}")).collect::<String>()),
        }
    }
}

/// A total order over values (SQLite's sort order, exact), to put two answers' rows side by side.
fn order(a: &V, b: &V) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match a.rank().cmp(&b.rank()) {
        Ordering::Equal => match (a, b) {
            (V::Int(x), V::Int(y)) => x.cmp(y),
            // equal numbers: the INTEGER first, so two answers holding the same values sort alike
            (V::Int(x), V::Real(y)) => (*x as f64).total_cmp(y).then(Ordering::Less),
            (V::Real(x), V::Int(y)) => x.total_cmp(&(*y as f64)).then(Ordering::Greater),
            (V::Real(x), V::Real(y)) => x.total_cmp(y),
            (V::Text(x), V::Text(y)) => x.cmp(y),
            (V::Blob(x), V::Blob(y)) => x.cmp(y),
            _ => Ordering::Equal,
        },
        o => o,
    }
}

fn row_order(a: &[V], b: &[V]) -> std::cmp::Ordering {
    a.iter().zip(b).map(|(x, y)| order(x, y)).find(|o| o.is_ne()).unwrap_or(a.len().cmp(&b.len()))
}

/// A value as a class of equal ones: numbers by value whatever their class, text folded as NOCASE
/// and RTRIM fold it. Text holding a NUL is NOCASE-equal to another of its length that agrees up
/// to that NUL (sqlite3StrNICmp stops there), so it folds to that prefix and its length.
fn folded(v: &V) -> V {
    match v {
        V::Int(i) => V::Real(*i as f64),
        // -0.0 and 0 are one value: a group may show either.
        V::Real(r) if *r == 0.0 => V::Real(0.0),
        V::Text(s) => match s.split_once('\0') {
            Some((head, _)) => V::Text(format!("{}\0{}", head.to_ascii_lowercase(), s.len())),
            None => V::Text(s.trim_end_matches(' ').to_ascii_lowercase()),
        },
        other => other.clone(),
    }
}

/// Whether two answers hold the same rows, as multisets (order aside); `loose` compares each value
/// as a class of equal ones: an answer that keeps one
/// of several equal values (DISTINCT, GROUP BY, UNION, MIN and MAX under a collation) may keep any
/// of them ('a' or 'A' under NOCASE, 1 or 1.0), and two engines may keep different ones.
pub fn same_rows_by(a: &Rows, b: &Rows, loose: bool) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let key = |r: &Vec<V>| -> Vec<V> { if loose { r.iter().map(folded).collect() } else { r.clone() } };
    let mut x: Vec<Vec<V>> = a.iter().map(key).collect();
    let mut y: Vec<Vec<V>> = b.iter().map(key).collect();
    x.sort_by(|p, q| row_order(p, q));
    y.sort_by(|p, q| row_order(p, q));
    x.iter().zip(&y).all(|(p, q)| p.len() == q.len() && p.iter().zip(q.iter()).all(|(u, v)| u.same(v)))
}

/// A short view of an answer, for a finding: its row count and first rows.
pub fn show(r: &Result<Rows, String>) -> String {
    show_up_to(r, 6)
}

/// Like [show], with up to `limit` rows: a finding keeps six, `--replay` shows them all.
pub fn show_up_to(r: &Result<Rows, String>, limit: usize) -> String {
    match r {
        Err(e) => format!("error: {e}"),
        Ok(rows) => {
            let mut s = format!("{} rows", rows.len());
            for row in rows.iter().take(limit) {
                s.push_str("\n    ");
                s.push_str(&row.iter().map(V::sql).collect::<Vec<_>>().join(", "));
            }
            if rows.len() > limit {
                s.push_str("\n    ...");
            }
            s
        }
    }
}

pub trait Engine {
    /// Runs one statement to its end; its rows, or the engine's error.
    fn run(&mut self, sql: &str) -> Result<Rows, String>;
}

// ---------------------------------------------------------------- Amphora

pub struct Amphora {
    io: Arc<dyn amphora_engine::IO>,
    pub conn: Arc<amphora_engine::Connection>,
    _db: Arc<amphora_engine::Database>,
}

impl Amphora {
    /// A new in-memory database; `n` names it apart from every other the process opens.
    pub fn new(n: u64) -> anyhow::Result<Self> {
        use amphora_engine::{ConnectOpts, Database, DatabaseOpts, MemoryIO, OpenOptions, SqliteDialect};
        let io: Arc<dyn amphora_engine::IO> = Arc::new(MemoryIO::new());
        let db = Database::open(io.clone(), &format!("oracle-{}-{n}.db", std::process::id()), OpenOptions::new(Arc::new(SqliteDialect)).db_opts(DatabaseOpts::new()))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let conn = db.connect(ConnectOpts::new()).map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(Self { io, conn, _db: db })
    }
}

fn amphora_value(v: &amphora_engine::Value) -> V {
    use amphora_engine::{Numeric, Value};
    match v {
        Value::Null => V::Null,
        Value::Numeric(Numeric::Integer(i)) => V::Int(*i),
        Value::Numeric(Numeric::Float(f)) => V::Real(f64::from(*f)),
        Value::Text(_) => V::Text(v.to_text().unwrap_or_default().to_string()),
        Value::Blob(_) => V::Blob(v.to_blob().unwrap_or_default().to_vec()),
    }
}

impl Engine for Amphora {
    fn run(&mut self, sql: &str) -> Result<Rows, String> {
        let mut stmt = self.conn.prepare(sql).map_err(|e| e.to_string())?;
        let mut rows = Vec::new();
        loop {
            match stmt.step().map_err(|e| e.to_string())? {
                amphora_engine::StepResult::Row => {
                    if let Some(r) = stmt.row() {
                        rows.push(r.get_values().map(amphora_value).collect());
                    }
                }
                amphora_engine::StepResult::Done => return Ok(rows),
                amphora_engine::StepResult::IO | amphora_engine::StepResult::Yield => self.io.step().map_err(|e| e.to_string())?,
                other => return Err(format!("step: {other:?}")),
            }
        }
    }
}

// ---------------------------------------------------------------- SQLite, the system's

/// The reference: a SQLite library loaded at run time and called through its C API, the system's
/// (libsqlite3.so.0, the release the correctness corpus is checked on, 3.54.0 here) unless
/// `use_library` names another: bench/oracles/sqlite.sh builds that release with the one option
/// Amphora adopts beyond the system's build. Not the bundled SQLite the bench links for its
/// benchmarks: an older release, it answers some statements otherwise (3.53.2 says `0 IN (SELECT
/// ...)` is FALSE over a column holding NULL, and `printf('')` is NULL). The bench exports no
/// symbols, so the two never resolve to each other.
mod ffi {
    use std::ffi::{c_char, c_int, c_void};

    pub type Open = unsafe extern "C" fn(*const c_char, *mut *mut c_void) -> c_int;
    pub type Close = unsafe extern "C" fn(*mut c_void) -> c_int;
    pub type Prepare = unsafe extern "C" fn(*mut c_void, *const c_char, c_int, *mut *mut c_void, *mut *const c_char) -> c_int;
    pub type Stmt = unsafe extern "C" fn(*mut c_void) -> c_int;
    pub type Col = unsafe extern "C" fn(*mut c_void, c_int) -> c_int;
    pub type ColI64 = unsafe extern "C" fn(*mut c_void, c_int) -> i64;
    pub type ColF64 = unsafe extern "C" fn(*mut c_void, c_int) -> f64;
    pub type ColPtr = unsafe extern "C" fn(*mut c_void, c_int) -> *const c_void;
    pub type Msg = unsafe extern "C" fn(*mut c_void) -> *const c_char;

    pub struct Api {
        pub open: Open,
        pub close: Close,
        pub prepare: Prepare,
        pub step: Stmt,
        pub finalize: Stmt,
        pub count: Stmt,
        pub kind: Col,
        pub int64: ColI64,
        pub double: ColF64,
        pub blob: ColPtr,
        pub text: ColPtr,
        pub bytes: Col,
        pub errmsg: Msg,
    }

    pub fn load(path: &str) -> Result<(libloading::Library, Api), String> {
        // SAFETY: loading a SQLite library runs its initializers, which only set up its own state;
        // each symbol is SQLite's documented C API with the signature its type above gives.
        unsafe {
            let lib = libloading::Library::new(path).map_err(|e| format!("loading SQLite from {path}: {e}"))?;
            let api = Api {
                open: *lib.get(b"sqlite3_open\0").map_err(|e| e.to_string())?,
                close: *lib.get(b"sqlite3_close_v2\0").map_err(|e| e.to_string())?,
                prepare: *lib.get(b"sqlite3_prepare_v2\0").map_err(|e| e.to_string())?,
                step: *lib.get(b"sqlite3_step\0").map_err(|e| e.to_string())?,
                finalize: *lib.get(b"sqlite3_finalize\0").map_err(|e| e.to_string())?,
                count: *lib.get(b"sqlite3_column_count\0").map_err(|e| e.to_string())?,
                kind: *lib.get(b"sqlite3_column_type\0").map_err(|e| e.to_string())?,
                int64: *lib.get(b"sqlite3_column_int64\0").map_err(|e| e.to_string())?,
                double: *lib.get(b"sqlite3_column_double\0").map_err(|e| e.to_string())?,
                blob: *lib.get(b"sqlite3_column_blob\0").map_err(|e| e.to_string())?,
                text: *lib.get(b"sqlite3_column_text\0").map_err(|e| e.to_string())?,
                bytes: *lib.get(b"sqlite3_column_bytes\0").map_err(|e| e.to_string())?,
                errmsg: *lib.get(b"sqlite3_errmsg\0").map_err(|e| e.to_string())?,
            };
            Ok((lib, api))
        }
    }
}

static SQLITE: std::sync::OnceLock<Result<(libloading::Library, ffi::Api), String>> = std::sync::OnceLock::new();
static LIBRARY: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The SQLite library to load in place of the system's; set before the first `Sqlite::new`.
pub fn use_library(path: String) -> Result<(), String> {
    LIBRARY.set(path).map_err(|p| format!("the SQLite library is already chosen; not {p}"))
}

fn api() -> Result<&'static ffi::Api, String> {
    match SQLITE.get_or_init(|| ffi::load(LIBRARY.get().map_or("libsqlite3.so.0", String::as_str))) {
        Ok((_, api)) => Ok(api),
        Err(e) => Err(e.clone()),
    }
}

pub struct Sqlite {
    db: *mut std::ffi::c_void,
}

impl Sqlite {
    pub fn new() -> anyhow::Result<Self> {
        let api = api().map_err(|e| anyhow::anyhow!(e))?;
        let mut db = std::ptr::null_mut();
        // SAFETY: a NUL-terminated name and a place for the handle, as sqlite3_open takes them.
        let rc = unsafe { (api.open)(c":memory:".as_ptr(), &mut db) };
        anyhow::ensure!(rc == 0 && !db.is_null(), "opening the system's SQLite: code {rc}");
        Ok(Self { db })
    }

    fn error(&self, api: &ffi::Api) -> String {
        // SAFETY: errmsg on an open handle returns a NUL-terminated string SQLite owns.
        unsafe { std::ffi::CStr::from_ptr((api.errmsg)(self.db)).to_string_lossy().into_owned() }
    }
}

impl Drop for Sqlite {
    fn drop(&mut self) {
        if let Ok(api) = api() {
            // SAFETY: the handle opened in new(), closed once; close_v2 waits for no statement
            // (each run finalizes its own).
            unsafe { (api.close)(self.db) };
        }
    }
}

impl Engine for Sqlite {
    fn run(&mut self, sql: &str) -> Result<Rows, String> {
        let api = api()?;
        let text = std::ffi::CString::new(sql).map_err(|_| "a NUL in the statement".to_owned())?;
        let mut stmt = std::ptr::null_mut();
        let mut tail = std::ptr::null();
        // SAFETY: an open handle, a NUL-terminated statement (length -1: to its NUL), and places for
        // the statement and the rest of the text.
        let rc = unsafe { (api.prepare)(self.db, text.as_ptr(), -1, &mut stmt, &mut tail) };
        if rc != 0 {
            return Err(self.error(api));
        }
        if stmt.is_null() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        // SAFETY (each call below): `stmt` is the statement prepared above, finalized once at the
        // end; a column's pointer is read for its byte count before the next step moves it.
        let n = unsafe { (api.count)(stmt) };
        loop {
            match unsafe { (api.step)(stmt) } {
                100 => {
                    let mut row = Vec::with_capacity(n as usize);
                    for i in 0..n {
                        row.push(unsafe {
                            match (api.kind)(stmt, i) {
                                1 => V::Int((api.int64)(stmt, i)),
                                2 => V::Real((api.double)(stmt, i)),
                                3 => {
                                    let p = (api.text)(stmt, i) as *const u8;
                                    let len = (api.bytes)(stmt, i) as usize;
                                    if p.is_null() {
                                        V::Text(String::new())
                                    } else {
                                        match std::str::from_utf8(std::slice::from_raw_parts(p, len)) {
                                            Ok(t) => V::Text(t.to_owned()),
                                            Err(_) => {
                                                (api.finalize)(stmt);
                                                return Err(ILL_FORMED.to_owned());
                                            }
                                        }
                                    }
                                }
                                4 => {
                                    let p = (api.blob)(stmt, i) as *const u8;
                                    let len = (api.bytes)(stmt, i) as usize;
                                    V::Blob(if p.is_null() { Vec::new() } else { std::slice::from_raw_parts(p, len).to_vec() })
                                }
                                _ => V::Null,
                            }
                        });
                    }
                    out.push(row);
                }
                101 => break,
                _ => {
                    let e = self.error(api);
                    unsafe { (api.finalize)(stmt) };
                    return Err(e);
                }
            }
        }
        unsafe { (api.finalize)(stmt) };
        Ok(out)
    }
}
