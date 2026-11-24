//! PostgreSQL 19's analysis (docs/design/postgres.md section 4, section 11's unit 8): a raw parse tree
//! becomes a typed one, its names, operators, functions and casts resolved as 19's `parse_*.c` resolve
//! them, over 19's own `pg_type`, `pg_operator`, `pg_proc` and `pg_cast` rows (amphora-pg-catalog). The
//! values and functions are those of int4, int8, text, char(n), "char", timestamp, timestamptz and
//! regclass, and name's input and output; a statement that needs another refuses with `0A000` naming
//! it. Pure: no I/O, no clock. Source lines are REL_19_0's.
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        clippy::disallowed_macros
    )
)]

pub mod binary;
pub mod coerce;
pub mod datetime;
pub mod expr;
pub mod func;
pub mod lower;
pub mod resolve;
pub mod stmt;
pub mod tz;
pub mod utility;

pub use expr::{Expr, ExprId, ExprKind, Exprs, Form};
pub use func::{Value, call, eval, input, output};
pub use lower::{Field, Lowered, fields, lower};
pub use stmt::{Query, Target, analyze};
pub use utility::{Utility, utility};

use amphora_pg_catalog::{SearchPath, data::type_oid, type_by_oid};

/// SQLSTATEs (src/backend/utils/errcodes.txt).
pub mod sqlstate {
    pub const FEATURE_NOT_SUPPORTED: &str = "0A000";
    pub const NUMERIC_VALUE_OUT_OF_RANGE: &str = "22003";
    pub const INVALID_DATETIME_FORMAT: &str = "22007";
    pub const DATETIME_FIELD_OVERFLOW: &str = "22008";
    pub const CHARACTER_NOT_IN_REPERTOIRE: &str = "22021";
    pub const PROTOCOL_VIOLATION: &str = "08P01";
    pub const NAME_TOO_LONG: &str = "42622";
    pub const INVALID_TIME_ZONE_DISPLACEMENT: &str = "22009";
    pub const DIVISION_BY_ZERO: &str = "22012";
    pub const STRING_DATA_RIGHT_TRUNCATION: &str = "22001";
    pub const INVALID_PARAMETER_VALUE: &str = "22023";
    pub const INVALID_TEXT_REPRESENTATION: &str = "22P02";
    pub const INVALID_SCHEMA_NAME: &str = "3F000";
    pub const INVALID_NAME: &str = "42602";
    pub const SYNTAX_ERROR: &str = "42601";
    pub const DUPLICATE_COLUMN: &str = "42701";
    pub const UNDEFINED_COLUMN: &str = "42703";
    pub const UNDEFINED_OBJECT: &str = "42704";
    pub const AMBIGUOUS_FUNCTION: &str = "42725";
    pub const DATATYPE_MISMATCH: &str = "42804";
    pub const WRONG_OBJECT_TYPE: &str = "42809";
    pub const CANNOT_COERCE: &str = "42846";
    pub const UNDEFINED_FUNCTION: &str = "42883";
    pub const UNDEFINED_TABLE: &str = "42P01";
    pub const DUPLICATE_TABLE: &str = "42P07";
    pub const UNDEFINED_PARAMETER: &str = "42P02";
    pub const AMBIGUOUS_PARAMETER: &str = "42P08";
    pub const INDETERMINATE_DATATYPE: &str = "42P18";
    pub const INTERNAL_ERROR: &str = "XX000";
}

/// 19's refusal: its SQLSTATE and message, and the byte of the statement it points at (-1 for none).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub sqlstate: &'static str,
    pub message: String,
    pub location: i32,
    /// Whether 19 raises it as the statement runs: the planner, which folds what is immutable, doesn't
    /// compute what this is of.
    pub runs: bool,
}

impl Error {
    pub fn new(sqlstate: &'static str, message: String) -> Self {
        Self {
            sqlstate,
            message,
            location: -1,
            runs: false,
        }
    }

    /// This refusal, raised as the statement runs.
    fn of_its_run(self) -> Self {
        Self { runs: true, ..self }
    }

    /// This refusal, pointing at `location` unless it points somewhere already.
    pub fn at(mut self, location: i32) -> Self {
        if self.location < 0 {
            self.location = location;
        }
        self
    }

    /// What this front end doesn't have yet.
    pub fn unsupported(what: &str) -> Self {
        Self::new(
            sqlstate::FEATURE_NOT_SUPPORTED,
            format!("{what} is not supported yet"),
        )
    }

    fn internal(message: &str) -> Self {
        Self::new(sqlstate::INTERNAL_ERROR, message.to_owned())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

impl From<amphora_pg_parser::tree::TreeFault> for Error {
    fn from(_: amphora_pg_parser::tree::TreeFault) -> Self {
        Self::internal("a parse tree node is missing")
    }
}

/// A table or catalog column, as `pg_attribute` has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub type_oid: u32,
    pub typmod: i32,
    pub not_null: bool,
}

/// What analysis asks of the schema.
pub trait Relations {
    /// `get_namespace_oid`: the schema of that name.
    fn namespace(&self, name: &str) -> Option<u32>;
    /// `get_relname_relid`: the relation of that name in that schema.
    fn relation(&self, namespace: u32, name: &str) -> Option<u32>;
    /// A relation's schema and name.
    fn name(&self, oid: u32) -> Option<(u32, String)>;
    /// `get_namespace_name`: a schema's name.
    fn namespace_name(&self, oid: u32) -> Option<String>;
    /// A relation's user columns, in `attnum` order.
    fn columns(&self, oid: u32) -> Vec<Column>;
}

/// What a session brings to its statements' analysis.
pub struct Session<'a> {
    pub path: &'a SearchPath,
    pub relations: &'a dyn Relations,
    /// The `TimeZone` setting.
    pub time_zone: &'a datetime::TimeZone,
    /// When the statement's transaction began, as microseconds from 2000-01-01 UTC
    /// (`GetCurrentTransactionStartTimestamp`, CURRENT_TIMESTAMP's value): none where no transaction runs.
    pub now: Option<i64>,
}

/// `format_type_be` (src/backend/utils/adt/format_type.c): a type's name as messages show it.
pub fn format_type(oid: u32) -> String {
    let element = coerce::element_type(oid);
    let name = |oid: u32| {
        let system = match oid {
            type_oid::BIT => "bit",
            type_oid::BOOL => "boolean",
            type_oid::BPCHAR => "character",
            type_oid::FLOAT4 => "real",
            type_oid::FLOAT8 => "double precision",
            type_oid::INT2 => "smallint",
            type_oid::INT4 => "integer",
            type_oid::INT8 => "bigint",
            type_oid::NUMERIC => "numeric",
            type_oid::INTERVAL => "interval",
            type_oid::TIME => "time without time zone",
            type_oid::TIMETZ => "time with time zone",
            type_oid::TIMESTAMP => "timestamp without time zone",
            type_oid::TIMESTAMPTZ => "timestamp with time zone",
            type_oid::VARBIT => "bit varying",
            type_oid::VARCHAR => "character varying",
            type_oid::CHAR => "\"char\"",
            type_oid::ANY => "\"any\"",
            0 => "-",
            _ => return type_by_oid(oid).map_or_else(|| "???".to_owned(), |t| t.name.to_owned()),
        };
        system.to_owned()
    };
    if element == 0 {
        name(oid)
    } else {
        format!("{}[]", name(element))
    }
}
