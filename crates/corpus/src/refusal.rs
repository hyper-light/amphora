//! Refusals compared by what kind of error they are (G2 step 4). A `statement error` record says
//! only that SQLite refuses the statement; the corpus keeps SQLite's error for each such record when
//! it is built (bench/corpus/dialects.py writes `<pool>/refusals/<file>.jsonl`, a JSON line per
//! refused record: `{"line": n, "code": SQLite's extended result code or null, "message": ...}`),
//! and an engine's refusal of the same record is read for the kind its own documented codes give
//! it. Two counts come of it: "any error matches" (the control: what sqllogictest itself asks) and
//! "the same kind of error" (an engine that refuses a UNIQUE violation as a syntax error doesn't
//! refuse it for SQLite's reason).
//!
//! The kinds are the ones every engine's codes name: a constraint violated, a statement that
//! doesn't parse, an object that doesn't exist, one that exists already, and a value of the wrong
//! type. Anything else is `Other`, never compared: an engine's error the mapping doesn't name says
//! nothing about its reason. Each engine's codes, by its documentation:
//! - SQLite: its result codes (<https://www.sqlite.org/rescode.html>): SQLITE_CONSTRAINT (19) and
//!   its extended codes are a constraint's, except SQLITE_CONSTRAINT_DATATYPE (3091, a STRICT
//!   column's type: "a value of the wrong type", <https://www.sqlite.org/stricttables.html>);
//!   SQLITE_MISMATCH (20, "datatype mismatch") is a type's. SQLITE_ERROR (1) is "a generic error
//!   code that is used when no other more specific error code is available", so the error's
//!   message says which: SQLite's own "syntax error", "incomplete input" and "unrecognized token"
//!   (tokenize.c, the parser's yy_syntax_error); "no such table|column|function|index|view|
//!   collation sequence|module|savepoint" (select.c, resolve.c, build.c); "already exists"
//!   (build.c's "table %T already exists", "index %s already exists"); "duplicate column name".
//! - PostgreSQL (OrioleDB and CedarDB speak it): its SQLSTATE (Appendix A, "PostgreSQL Error
//!   Codes", <https://www.postgresql.org/docs/current/errcodes-appendix.html>): class 23 "Integrity
//!   Constraint Violation"; 42601 syntax_error; 42P01 undefined_table, 42703 undefined_column,
//!   42883 undefined_function, 42704 undefined_object, 3F000 invalid_schema_name; 42P07
//!   duplicate_table, 42701 duplicate_column, 42710 duplicate_object, 42P06 duplicate_schema;
//!   class 22 "Data Exception", 42804 datatype_mismatch and 42846 cannot_coerce: a type's. psql
//!   writes it with `VERBOSITY=verbose` ("ERROR:  42601: syntax error ...", psql's documentation
//!   of VERBOSITY).
//! - MySQL: its server error numbers (MySQL 8.4 Error Reference, "Server Error Message Reference",
//!   <https://dev.mysql.com/doc/mysql-errors/8.4/en/server-error-reference.html>), which its client
//!   writes "ERROR 1062 (23000) at line 1: ...": 1062 ER_DUP_ENTRY, 1586
//!   ER_DUP_ENTRY_WITH_KEY_NAME, 1048 ER_BAD_NULL_ERROR, 1364 ER_NO_DEFAULT_FOR_FIELD, 1451
//!   ER_ROW_IS_REFERENCED_2, 1452 ER_NO_REFERENCED_ROW_2, 3819 ER_CHECK_CONSTRAINT_VIOLATED; 1064
//!   ER_PARSE_ERROR, 1149 ER_SYNTAX_ERROR; 1146 ER_NO_SUCH_TABLE, 1054 ER_BAD_FIELD_ERROR, 1305
//!   ER_SP_DOES_NOT_EXIST, 1051 ER_BAD_TABLE_ERROR, 1049 ER_BAD_DB_ERROR, 1091
//!   ER_CANT_DROP_FIELD_OR_KEY, 1176 ER_KEY_DOES_NOT_EXITS, 1273 ER_UNKNOWN_COLLATION; 1050
//!   ER_TABLE_EXISTS_ERROR, 1060 ER_DUP_FIELDNAME, 1061 ER_DUP_KEYNAME, 1007 ER_DB_CREATE_EXISTS,
//!   1304 ER_SP_ALREADY_EXISTS; 1264 ER_WARN_DATA_OUT_OF_RANGE, 1265 WARN_DATA_TRUNCATED, 1292
//!   ER_TRUNCATED_WRONG_VALUE, 1366 ER_TRUNCATED_WRONG_VALUE_FOR_FIELD, 1406 ER_DATA_TOO_LONG.
//! - ClickHouse: its error codes (src/Common/ErrorCodes.cpp,
//!   <https://github.com/ClickHouse/ClickHouse/blob/master/src/Common/ErrorCodes.cpp>), its HTTP
//!   answer "Code: 62. DB::Exception: ...": 469 VIOLATED_CONSTRAINT; 62 SYNTAX_ERROR; 60
//!   UNKNOWN_TABLE, 47 UNKNOWN_IDENTIFIER, 46 UNKNOWN_FUNCTION, 81 UNKNOWN_DATABASE, 16
//!   NO_SUCH_COLUMN_IN_TABLE, 10 NOT_FOUND_COLUMN_IN_BLOCK; 57 TABLE_ALREADY_EXISTS, 82
//!   DATABASE_ALREADY_EXISTS, 15 DUPLICATE_COLUMN; 53 TYPE_MISMATCH, 6 CANNOT_PARSE_TEXT, 27
//!   CANNOT_PARSE_INPUT_ASSERTION_FAILED, 38 CANNOT_PARSE_DATE, 41 CANNOT_PARSE_DATETIME, 43
//!   ILLEGAL_TYPE_OF_ARGUMENT, 70 CANNOT_CONVERT_TYPE, 72 CANNOT_PARSE_NUMBER, 386 NO_COMMON_TYPE.
//! - DuckDB: its error types (the ExceptionType names an error starts with, "<Type> Error: ...",
//!   src/common/exception.cpp's EXCEPTION_MAP,
//!   <https://github.com/duckdb/duckdb/blob/main/src/common/exception.cpp>): "Constraint Error";
//!   "Parser Error" and "Syntax Error"; "Conversion Error", "Mismatch Type Error" and "Out of Range
//!   Error" (a value outside its type's range) a type's. "Catalog Error" holds both an object that
//!   doesn't exist ("... does not exist!") and one that does ("... already exists!"), and "Binder
//!   Error" a column not found ("Referenced column ... not found") among others, so their message
//!   says which, as SQLite's does for SQLITE_ERROR.
//! - Turso and Amphora write SQLite's messages (the code isn't in their text): SQLite's mapping by
//!   message.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What kind of error an engine's refusal is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// A constraint violated (UNIQUE, NOT NULL, CHECK, FOREIGN KEY, PRIMARY KEY).
    Constraint,
    /// The statement doesn't parse.
    Syntax,
    /// A table, column, function or other object that doesn't exist.
    Missing,
    /// An object that exists already.
    Exists,
    /// A value of the wrong type, or outside its type.
    Type,
    /// None the engine's codes name: never compared.
    Other,
}

impl ErrorKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Constraint => "constraint",
            Self::Syntax => "syntax",
            Self::Missing => "missing object",
            Self::Exists => "existing object",
            Self::Type => "type",
            Self::Other => "other",
        }
    }
}

/// SQLite's messages for SQLITE_ERROR's kinds, and for an error whose code isn't in its text.
fn sqlite_message(message: &str) -> ErrorKind {
    let m = message.to_ascii_lowercase();
    if m.contains("constraint failed") {
        // "UNIQUE constraint failed: t.a", "NOT NULL constraint failed", "CHECK constraint failed",
        // "FOREIGN KEY constraint failed" (sqlite3HaltConstraint's messages).
        ErrorKind::Constraint
    } else if m.contains("cannot store") || m.contains("datatype mismatch") {
        // SQLITE_CONSTRAINT_DATATYPE's "cannot store TEXT value in INTEGER column t.a", and
        // SQLITE_MISMATCH's "datatype mismatch".
        ErrorKind::Type
    } else if m.contains("syntax error")
        || m.contains("incomplete input")
        || m.contains("unrecognized token")
    {
        ErrorKind::Syntax
    } else if m.contains("no such ") || m.contains("unknown database") {
        ErrorKind::Missing
    } else if m.contains("already exists") || m.contains("duplicate column name") {
        ErrorKind::Exists
    } else {
        ErrorKind::Other
    }
}

/// SQLite's error by its extended result code, and its message when the code is SQLITE_ERROR's or
/// unknown.
pub fn sqlite(code: Option<i64>, message: &str) -> ErrorKind {
    match code {
        Some(3091) => ErrorKind::Type,
        Some(c) if c & 0xff == 19 => ErrorKind::Constraint,
        Some(c) if c & 0xff == 20 => ErrorKind::Type,
        Some(c) if c & 0xff == 1 => sqlite_message(message),
        Some(_) => ErrorKind::Other,
        None => sqlite_message(message),
    }
}

/// The digits right after `marker` in `text`, as a number.
fn number_after(text: &str, marker: &str) -> Option<i64> {
    let at = text.find(marker)?.checked_add(marker.len())?;
    let rest = text.get(at..)?.trim_start();
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn postgres(error: &str) -> ErrorKind {
    // "psql:<stdin>:3: ERROR:  23505: duplicate key value ..." (VERBOSITY verbose).
    let Some(state) = error
        .lines()
        .find_map(|l| l.split_once("ERROR:").map(|(_, r)| r.trim_start()))
        .and_then(|r| r.split(':').next())
        .map(str::trim)
        .filter(|s| {
            s.len() == 5
                && s.chars()
                    .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        })
    else {
        return ErrorKind::Other;
    };
    match state {
        "42601" => ErrorKind::Syntax,
        "42P01" | "42703" | "42883" | "42704" | "3F000" => ErrorKind::Missing,
        "42P07" | "42701" | "42710" | "42P06" => ErrorKind::Exists,
        "42804" | "42846" => ErrorKind::Type,
        s if s.starts_with("23") => ErrorKind::Constraint,
        s if s.starts_with("22") => ErrorKind::Type,
        _ => ErrorKind::Other,
    }
}

fn mysql(error: &str) -> ErrorKind {
    match number_after(error, "ERROR ") {
        Some(1062 | 1586 | 1048 | 1364 | 1451 | 1452 | 3819) => ErrorKind::Constraint,
        Some(1064 | 1149) => ErrorKind::Syntax,
        Some(1146 | 1054 | 1305 | 1051 | 1049 | 1091 | 1176 | 1273) => ErrorKind::Missing,
        Some(1050 | 1060 | 1061 | 1007 | 1304) => ErrorKind::Exists,
        Some(1264 | 1265 | 1292 | 1366 | 1406) => ErrorKind::Type,
        _ => ErrorKind::Other,
    }
}

fn clickhouse(error: &str) -> ErrorKind {
    match number_after(error, "Code:") {
        Some(469) => ErrorKind::Constraint,
        Some(62) => ErrorKind::Syntax,
        Some(60 | 47 | 46 | 81 | 16 | 10) => ErrorKind::Missing,
        Some(57 | 82 | 15) => ErrorKind::Exists,
        Some(53 | 6 | 27 | 38 | 41 | 43 | 70 | 72 | 386) => ErrorKind::Type,
        _ => ErrorKind::Other,
    }
}

fn duckdb(error: &str) -> ErrorKind {
    let line = error
        .lines()
        .find(|l| l.contains("Error: "))
        .unwrap_or(error);
    // Its older shells put "Error: " before the type ("Error: Parser Error: ...").
    let line = line
        .trim_start()
        .strip_prefix("Error: ")
        .unwrap_or(line.trim_start());
    let (kind, message) = line.split_once(" Error: ").unwrap_or((line, ""));
    let m = message.to_ascii_lowercase();
    match kind.trim() {
        "Constraint" => ErrorKind::Constraint,
        "Parser" | "Syntax" => ErrorKind::Syntax,
        "Conversion" | "Mismatch Type" | "Out of Range" => ErrorKind::Type,
        "Catalog" if m.contains("does not exist") => ErrorKind::Missing,
        "Catalog" if m.contains("already exists") => ErrorKind::Exists,
        "Binder" if m.contains("not found") || m.contains("does not exist") => ErrorKind::Missing,
        _ => ErrorKind::Other,
    }
}

/// SQLite's shell: "Runtime error near line 2: UNIQUE constraint failed: t.a (19)", its extended
/// code in brackets at the end; "Parse error near line 1: no such table: x" without one.
fn sqlite_shell(error: &str) -> ErrorKind {
    let line = error.lines().next().unwrap_or(error).trim_end();
    let code = line
        .strip_suffix(')')
        .and_then(|l| l.rsplit_once('('))
        .and_then(|(_, n)| n.parse().ok());
    sqlite(code, line)
}

/// The kind of `engine`'s refusal, `error` being its text as the engine's adapter holds it
/// (crate::run::Stop::Refused).
pub fn of(engine: &str, error: &str) -> ErrorKind {
    match engine {
        "postgres" | "orioledb" | "cedardb" => postgres(error),
        "mysql" => mysql(error),
        "clickhouse" => clickhouse(error),
        "duckdb" => duckdb(error),
        "sqlite" => sqlite_shell(error),
        _ => sqlite(None, error),
    }
}

/// Where `<pool>/<rel>`'s refusals are.
pub fn path(pool: &Path, rel: &Path) -> PathBuf {
    let mut p = pool.join("refusals").join(rel);
    let mut name = p.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".jsonl");
    p.set_file_name(name);
    p
}

/// SQLite's error for each `statement error` record of one file, by the line it starts on.
#[derive(Clone, Debug, Default)]
pub struct Refusals {
    by_line: HashMap<usize, ErrorKind>,
}

#[derive(serde::Deserialize)]
struct Line {
    line: usize,
    code: Option<i64>,
    message: String,
}

impl Refusals {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut by_line = HashMap::new();
        for (n, l) in text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
        {
            let l: Line = serde_json::from_str(l)
                .map_err(|e| format!("line {}: {e}", n.saturating_add(1)))?;
            by_line.insert(l.line, sqlite(l.code, &l.message));
        }
        Ok(Self { by_line })
    }

    /// The file's refusals; none when the pool has no such file (one built before refusals were
    /// kept: every refusal is then counted as of no known kind, never as the same kind).
    pub fn read(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// SQLite's kind for the record at `line`, None when it wasn't kept.
    pub fn get(&self, line: usize) -> Option<ErrorKind> {
        self.by_line.get(&line).copied()
    }
}

/// How an engine's refusal of a record SQLite refuses compares with SQLite's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compared {
    /// The same kind.
    Same,
    /// Another kind (the engine's `Other` included: it refused for a reason its codes don't name).
    Differs {
        sqlite: ErrorKind,
        engine: ErrorKind,
    },
    /// SQLite's kind wasn't kept, or is `Other`: not compared.
    Unknown,
}

pub fn compare(sqlite: Option<ErrorKind>, engine: ErrorKind) -> Compared {
    match sqlite {
        None | Some(ErrorKind::Other) => Compared::Unknown,
        Some(s) if s == engine => Compared::Same,
        Some(s) => Compared::Differs { sqlite: s, engine },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Do: each engine's error text for the same five refusals, as each writes it. Expect: the
    /// same kind from each, by its own codes.
    #[test]
    fn each_engines_codes_give_the_shared_kinds() {
        let cases: [(&str, &str, ErrorKind); 27] = [
            (
                "sqlite",
                "Runtime error near line 2: UNIQUE constraint failed: t.a (2067)",
                ErrorKind::Constraint,
            ),
            (
                "sqlite",
                "Parse error near line 1: near \"SELEC\": syntax error",
                ErrorKind::Syntax,
            ),
            (
                "sqlite",
                "Parse error near line 1: no such table: nope",
                ErrorKind::Missing,
            ),
            (
                "sqlite",
                "Parse error near line 1: table t already exists",
                ErrorKind::Exists,
            ),
            (
                "sqlite",
                "Runtime error near line 3: cannot store TEXT value in INTEGER column st.k (3091)",
                ErrorKind::Type,
            ),
            (
                "postgres",
                "psql:<stdin>:3: ERROR:  23505: duplicate key value violates unique constraint \"t_pkey\"",
                ErrorKind::Constraint,
            ),
            (
                "postgres",
                "psql:<stdin>:3: ERROR:  42601: syntax error at or near \"SELEC\"",
                ErrorKind::Syntax,
            ),
            (
                "cedardb",
                "psql:<stdin>:3: ERROR:  42P01: relation \"nope\" does not exist",
                ErrorKind::Missing,
            ),
            (
                "orioledb",
                "psql:<stdin>:3: ERROR:  42P07: relation \"t\" already exists",
                ErrorKind::Exists,
            ),
            (
                "postgres",
                "psql:<stdin>:3: ERROR:  22P02: invalid input syntax for type integer: \"abc\"",
                ErrorKind::Type,
            ),
            (
                "mysql",
                "ERROR 1062 (23000) at line 3: Duplicate entry '1' for key 't.PRIMARY'",
                ErrorKind::Constraint,
            ),
            (
                "mysql",
                "ERROR 1064 (42000) at line 3: You have an error in your SQL syntax",
                ErrorKind::Syntax,
            ),
            (
                "mysql",
                "ERROR 1146 (42S02) at line 3: Table 'db.nope' doesn't exist",
                ErrorKind::Missing,
            ),
            (
                "mysql",
                "ERROR 1050 (42S01) at line 3: Table 't' already exists",
                ErrorKind::Exists,
            ),
            (
                "mysql",
                "ERROR 1366 (HY000) at line 3: Incorrect integer value: 'abc'",
                ErrorKind::Type,
            ),
            (
                "clickhouse",
                "Code: 469. DB::Exception: Constraint `c` for table t is violated",
                ErrorKind::Constraint,
            ),
            (
                "clickhouse",
                "Code: 62. DB::Exception: Syntax error: failed at position 1",
                ErrorKind::Syntax,
            ),
            (
                "clickhouse",
                "Code: 60. DB::Exception: Table db.nope does not exist. (UNKNOWN_TABLE)",
                ErrorKind::Missing,
            ),
            (
                "clickhouse",
                "Code: 57. DB::Exception: Table db.t already exists. (TABLE_ALREADY_EXISTS)",
                ErrorKind::Exists,
            ),
            (
                "clickhouse",
                "Code: 6. DB::Exception: Cannot parse string 'abc' as Int64",
                ErrorKind::Type,
            ),
            (
                "duckdb",
                "Constraint Error: Duplicate key \"k: 1\" violates primary key constraint.",
                ErrorKind::Constraint,
            ),
            (
                "duckdb",
                "Parser Error: syntax error at or near \"SELEC\"",
                ErrorKind::Syntax,
            ),
            (
                "duckdb",
                "Catalog Error: Table with name nope does not exist!",
                ErrorKind::Missing,
            ),
            (
                "duckdb",
                "Catalog Error: Table with name t already exists!",
                ErrorKind::Exists,
            ),
            (
                "duckdb",
                "Conversion Error: Could not convert string 'abc' to INT32",
                ErrorKind::Type,
            ),
            (
                "turso",
                "  × Parse error: no such table: nope",
                ErrorKind::Missing,
            ),
            (
                "amphora",
                "UNIQUE constraint failed: t.a",
                ErrorKind::Constraint,
            ),
        ];
        for (engine, error, want) in cases {
            assert_eq!(of(engine, error), want, "{engine}: {error}");
        }
    }

    /// Do: SQLite's codes as the corpus keeps them: an extended constraint code, STRICT's datatype
    /// code, SQLITE_ERROR with its messages, and a code no kind names. Expect: their kinds, the
    /// last Other.
    #[test]
    fn sqlite_codes_and_its_generic_errors_messages() {
        assert_eq!(
            sqlite(Some(1555), "UNIQUE constraint failed: t.k"),
            ErrorKind::Constraint
        );
        assert_eq!(
            sqlite(Some(3091), "cannot store TEXT value in INTEGER column"),
            ErrorKind::Type
        );
        assert_eq!(
            sqlite(Some(1), "near \"x\": syntax error"),
            ErrorKind::Syntax
        );
        assert_eq!(sqlite(Some(1), "no such column: q"), ErrorKind::Missing);
        assert_eq!(
            sqlite(Some(1), "too many terms in compound SELECT"),
            ErrorKind::Other
        );
        assert_eq!(sqlite(Some(18), "string or blob too big"), ErrorKind::Other);
    }

    /// Do: refusals kept for two lines, one of no named kind, and an engine's refusals of each and
    /// of a line not kept. Expect: same, differs, and unknown for the two not compared.
    #[test]
    fn kept_refusals_compare_by_kind() {
        let kept = Refusals::parse(
            "{\"line\": 3, \"code\": 2067, \"message\": \"UNIQUE constraint failed: t.a\"}\n{\"line\": 9, \"code\": 1, \"message\": \"too many columns\"}\n\n{\"line\": 12, \"code\": null, \"message\": \"no such table: x\"}\n",
        )
        .unwrap();
        assert_eq!(compare(kept.get(3), ErrorKind::Constraint), Compared::Same);
        assert_eq!(
            compare(kept.get(12), ErrorKind::Syntax),
            Compared::Differs {
                sqlite: ErrorKind::Missing,
                engine: ErrorKind::Syntax
            }
        );
        assert_eq!(compare(kept.get(9), ErrorKind::Syntax), Compared::Unknown);
        assert_eq!(compare(kept.get(5), ErrorKind::Syntax), Compared::Unknown);
        assert!(Refusals::parse("{\"line\": 1}").is_err());
    }
}
