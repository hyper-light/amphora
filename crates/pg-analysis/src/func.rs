//! Values, and the functions of this unit's types: each built-in function's body under its `prosrc`,
//! written from PostgreSQL's C (REL_19_0 src/backend/utils/adt: int.c, int8.c, bool.c, char.c, oid.c,
//! regproc.c, timestamp.c, varchar.c, varlena.c, name.c, array_userfuncs.c), raising its SQLSTATEs. Text compares by its bytes,
//! as the `C` and builtin `C.UTF-8` collations do (docs/design/postgres.md section 5).

use std::cmp::Ordering;

use amphora_pg_catalog::data::type_oid;
use amphora_pg_catalog::{Kind, Proc, type_by_oid};
use amphora_pg_parser::tree::BoolOp;

use crate::array::{self, Array};
use crate::binary::NAMEDATALEN;
use crate::coerce::element_type;
use crate::expr::{ExprId, ExprKind, Exprs};
use crate::{Error, Session, datetime, format_type, sqlstate};

/// A value that isn't NULL; its type is its expression's.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    /// `"char"`.
    Char(u8),
    Int4(i32),
    Int8(i64),
    /// `oid` and `regclass`.
    Oid(u32),
    /// `text`, `name` and `bpchar`, a `char(n)` with its padding.
    Text(String),
    /// `timestamp` and `timestamptz`: microseconds from 2000-01-01, the extremes the infinities.
    Timestamp(i64),
    /// A numeric constant's text: no function takes it yet.
    Numeric(String),
    /// An unknown literal's text.
    Unknown(String),
    Array(Array),
}

/// A strict function's body, over values that aren't NULL.
type Body = fn(&[Value], &Session<'_>) -> Result<Value, Error>;

/// The body of a function that takes a NULL.
type OpenBody = fn(&[Option<Value>]) -> Result<Option<Value>, Error>;

/// Whether a function can be called: its body is here, or it is an I/O function [`input`] or
/// [`output`] has.
pub fn has_body(function: &Proc) -> bool {
    body(function.src).is_some()
        || open_body(function.src).is_some()
        || matches!(
            function.src,
            "array_in"
                | "array_out"
                | "boolin"
                | "boolout"
                | "charin"
                | "charout"
                | "int4in"
                | "int4out"
                | "int8in"
                | "int8out"
                | "oidin"
                | "oidout"
                | "regclassin"
                | "regclassout"
                | "textin"
                | "textout"
                | "namein"
                | "nameout"
                | "bpcharin"
                | "bpcharout"
                | "timestamp_in"
                | "timestamp_out"
                | "timestamptz_in"
                | "timestamptz_out"
        )
}

fn out_of_range(what: &str) -> Error {
    Error::new(
        sqlstate::NUMERIC_VALUE_OUT_OF_RANGE,
        format!("{what} out of range"),
    )
}

fn division_by_zero() -> Error {
    Error::new(sqlstate::DIVISION_BY_ZERO, "division by zero".to_owned())
}

fn mismatch() -> Error {
    Error::internal("a function got a value of another type")
}

fn int4(v: Option<&Value>) -> Result<i32, Error> {
    match v {
        Some(Value::Int4(i)) => Ok(*i),
        _ => Err(mismatch()),
    }
}

/// An int4 or int8 as int8: the cross-type operators widen before they work.
fn int8(v: Option<&Value>) -> Result<i64, Error> {
    match v {
        Some(Value::Int4(i)) => Ok(i64::from(*i)),
        Some(Value::Int8(i)) => Ok(*i),
        _ => Err(mismatch()),
    }
}

fn text(v: Option<&Value>) -> Result<&str, Error> {
    match v {
        Some(Value::Text(s)) => Ok(s),
        _ => Err(mismatch()),
    }
}

fn oid(v: Option<&Value>) -> Result<u32, Error> {
    match v {
        Some(Value::Oid(o)) => Ok(*o),
        _ => Err(mismatch()),
    }
}

fn byte(v: Option<&Value>) -> Result<u8, Error> {
    match v {
        Some(Value::Char(c)) => Ok(*c),
        _ => Err(mismatch()),
    }
}

fn boolean(v: Option<&Value>) -> Result<bool, Error> {
    match v {
        Some(Value::Bool(b)) => Ok(*b),
        _ => Err(mismatch()),
    }
}

fn timestamp(v: Option<&Value>) -> Result<i64, Error> {
    match v {
        Some(Value::Timestamp(t)) => Ok(*t),
        _ => Err(mismatch()),
    }
}

fn int4_op(a: &[Value], op: fn(i32, i32) -> Option<i32>) -> Result<Value, Error> {
    op(int4(a.first())?, int4(a.get(1))?)
        .map(Value::Int4)
        .ok_or_else(|| out_of_range("integer"))
}

fn int8_op(a: &[Value], op: fn(i64, i64) -> Option<i64>) -> Result<Value, Error> {
    op(int8(a.first())?, int8(a.get(1))?)
        .map(Value::Int8)
        .ok_or_else(|| out_of_range("bigint"))
}

fn int4_div(a: &[Value], op: fn(i32, i32) -> Option<i32>) -> Result<Value, Error> {
    match int4(a.get(1))? {
        0 => Err(division_by_zero()),
        _ => int4_op(a, op),
    }
}

fn int8_div(a: &[Value], op: fn(i64, i64) -> Option<i64>) -> Result<Value, Error> {
    match int8(a.get(1))? {
        0 => Err(division_by_zero()),
        _ => int8_op(a, op),
    }
}

fn ints(a: &[Value], holds: fn(Ordering) -> bool) -> Result<Value, Error> {
    Ok(Value::Bool(holds(int8(a.first())?.cmp(&int8(a.get(1))?))))
}

fn oids(a: &[Value], holds: fn(Ordering) -> bool) -> Result<Value, Error> {
    Ok(Value::Bool(holds(oid(a.first())?.cmp(&oid(a.get(1))?))))
}

fn chars(a: &[Value], holds: fn(Ordering) -> bool) -> Result<Value, Error> {
    Ok(Value::Bool(holds(byte(a.first())?.cmp(&byte(a.get(1))?))))
}

fn bools(a: &[Value], holds: fn(Ordering) -> bool) -> Result<Value, Error> {
    Ok(Value::Bool(holds(
        boolean(a.first())?.cmp(&boolean(a.get(1))?),
    )))
}

fn texts(a: &[Value], holds: fn(Ordering) -> bool) -> Result<Value, Error> {
    Ok(Value::Bool(holds(text(a.first())?.cmp(text(a.get(1))?))))
}

/// `bcTruelen` (varchar.c:664): bpchars compare without their trailing spaces.
fn bpchars(a: &[Value], holds: fn(Ordering) -> bool) -> Result<Value, Error> {
    let trimmed = |v| Ok::<_, Error>(text(v)?.trim_end_matches(' '));
    Ok(Value::Bool(holds(
        trimmed(a.first())?.cmp(trimmed(a.get(1))?),
    )))
}

fn times(a: &[Value], holds: fn(Ordering) -> bool) -> Result<Value, Error> {
    Ok(Value::Bool(holds(
        timestamp(a.first())?.cmp(&timestamp(a.get(1))?),
    )))
}

fn local_first(a: &[Value], s: &Session<'_>, holds: fn(Ordering) -> bool) -> Result<Value, Error> {
    let order = datetime::compare(timestamp(a.first())?, timestamp(a.get(1))?, s.time_zone);
    Ok(Value::Bool(holds(order)))
}

fn local_second(a: &[Value], s: &Session<'_>, holds: fn(Ordering) -> bool) -> Result<Value, Error> {
    let order = datetime::compare(timestamp(a.get(1))?, timestamp(a.first())?, s.time_zone);
    Ok(Value::Bool(holds(order.reverse())))
}

/// C's `arg1 << arg2` as PostgreSQL's supported machines shift: by the low bits of arg2.
fn bits(v: Option<&Value>) -> Result<u32, Error> {
    Ok(int4(v)?.cast_unsigned())
}

fn body(src: &str) -> Option<Body> {
    use Ordering as O;
    let body: Body = match src {
        "int4pl" => |a, _| int4_op(a, i32::checked_add),
        "int4mi" => |a, _| int4_op(a, i32::checked_sub),
        "int4mul" => |a, _| int4_op(a, i32::checked_mul),
        "int4div" => |a, _| int4_div(a, i32::checked_div),
        // int4mod (int.c:1143): a remainder by -1 is 0, overflow or not.
        "int4mod" => |a, _| int4_div(a, |x, y| Some(x.checked_rem(y).unwrap_or(0))),
        "int4up" | "int8up" => |a, _| a.first().cloned().ok_or_else(mismatch),
        "int4um" => |a, _| {
            int4_op(
                &[Value::Int4(0), Value::Int4(int4(a.first())?)],
                i32::checked_sub,
            )
        },
        "int4abs" => |a, _| {
            let abs = int4(a.first())?.checked_abs();
            abs.map(Value::Int4).ok_or_else(|| out_of_range("integer"))
        },
        "int4not" => |a, _| Ok(Value::Int4(!int4(a.first())?)),
        "int4and" => |a, _| int4_op(a, |x, y| Some(x & y)),
        "int4or" => |a, _| int4_op(a, |x, y| Some(x | y)),
        "int4xor" => |a, _| int4_op(a, |x, y| Some(x ^ y)),
        "int4shl" => |a, _| Ok(Value::Int4(int4(a.first())?.wrapping_shl(bits(a.get(1))?))),
        "int4shr" => |a, _| Ok(Value::Int4(int4(a.first())?.wrapping_shr(bits(a.get(1))?))),
        "int8pl" | "int48pl" | "int84pl" => |a, _| int8_op(a, i64::checked_add),
        "int8mi" | "int48mi" | "int84mi" => |a, _| int8_op(a, i64::checked_sub),
        "int8mul" | "int48mul" | "int84mul" => |a, _| int8_op(a, i64::checked_mul),
        "int8div" | "int48div" | "int84div" => |a, _| int8_div(a, i64::checked_div),
        "int8mod" => |a, _| int8_div(a, |x, y| Some(x.checked_rem(y).unwrap_or(0))),
        "int8um" => |a, _| {
            int8_op(
                &[Value::Int8(0), Value::Int8(int8(a.first())?)],
                i64::checked_sub,
            )
        },
        "int8abs" => |a, _| {
            let abs = int8(a.first())?.checked_abs();
            abs.map(Value::Int8).ok_or_else(|| out_of_range("bigint"))
        },
        "int8not" => |a, _| Ok(Value::Int8(!int8(a.first())?)),
        "int8and" => |a, _| int8_op(a, |x, y| Some(x & y)),
        "int8or" => |a, _| int8_op(a, |x, y| Some(x | y)),
        "int8xor" => |a, _| int8_op(a, |x, y| Some(x ^ y)),
        "int8shl" => |a, _| Ok(Value::Int8(int8(a.first())?.wrapping_shl(bits(a.get(1))?))),
        "int8shr" => |a, _| Ok(Value::Int8(int8(a.first())?.wrapping_shr(bits(a.get(1))?))),
        "int4eq" | "int8eq" | "int48eq" | "int84eq" => |a, _| ints(a, O::is_eq),
        "int4ne" | "int8ne" | "int48ne" | "int84ne" => |a, _| ints(a, O::is_ne),
        "int4lt" | "int8lt" | "int48lt" | "int84lt" => |a, _| ints(a, O::is_lt),
        "int4le" | "int8le" | "int48le" | "int84le" => |a, _| ints(a, O::is_le),
        "int4gt" | "int8gt" | "int48gt" | "int84gt" => |a, _| ints(a, O::is_gt),
        "int4ge" | "int8ge" | "int48ge" | "int84ge" => |a, _| ints(a, O::is_ge),
        "oideq" => |a, _| oids(a, O::is_eq),
        "oidne" => |a, _| oids(a, O::is_ne),
        "oidlt" => |a, _| oids(a, O::is_lt),
        "oidle" => |a, _| oids(a, O::is_le),
        "oidgt" => |a, _| oids(a, O::is_gt),
        "oidge" => |a, _| oids(a, O::is_ge),
        "chareq" => |a, _| chars(a, O::is_eq),
        "charne" => |a, _| chars(a, O::is_ne),
        "charlt" => |a, _| chars(a, O::is_lt),
        "charle" => |a, _| chars(a, O::is_le),
        "chargt" => |a, _| chars(a, O::is_gt),
        "charge" => |a, _| chars(a, O::is_ge),
        "booleq" => |a, _| bools(a, O::is_eq),
        "boolne" => |a, _| bools(a, O::is_ne),
        "boollt" => |a, _| bools(a, O::is_lt),
        "boolle" => |a, _| bools(a, O::is_le),
        "boolgt" => |a, _| bools(a, O::is_gt),
        "boolge" => |a, _| bools(a, O::is_ge),
        "texteq" | "nameeq" | "nameeqtext" | "texteqname" => |a, _| texts(a, O::is_eq),
        "textne" | "namene" | "namenetext" | "textnename" => |a, _| texts(a, O::is_ne),
        "text_lt" | "text_pattern_lt" | "namelt" | "namelttext" | "textltname" => {
            |a, _| texts(a, O::is_lt)
        }
        "text_le" | "text_pattern_le" | "namele" | "nameletext" | "textlename" => {
            |a, _| texts(a, O::is_le)
        }
        "text_gt" | "text_pattern_gt" | "namegt" | "namegttext" | "textgtname" => {
            |a, _| texts(a, O::is_gt)
        }
        "text_ge" | "text_pattern_ge" | "namege" | "namegetext" | "textgename" => {
            |a, _| texts(a, O::is_ge)
        }
        "text_starts_with" => |a, _| Ok(Value::Bool(text(a.first())?.starts_with(text(a.get(1))?))),
        "textcat" => |a, _| {
            let mut s = text(a.first())?.to_owned();
            s.push_str(text(a.get(1))?);
            Ok(Value::Text(s))
        },
        "bpchareq" => |a, _| bpchars(a, O::is_eq),
        "bpcharne" => |a, _| bpchars(a, O::is_ne),
        "bpcharlt" | "bpchar_pattern_lt" => |a, _| bpchars(a, O::is_lt),
        "bpcharle" | "bpchar_pattern_le" => |a, _| bpchars(a, O::is_le),
        "bpchargt" | "bpchar_pattern_gt" => |a, _| bpchars(a, O::is_gt),
        "bpcharge" | "bpchar_pattern_ge" => |a, _| bpchars(a, O::is_ge),
        "timestamp_eq" => |a, _| times(a, O::is_eq),
        "timestamp_ne" => |a, _| times(a, O::is_ne),
        "timestamp_lt" => |a, _| times(a, O::is_lt),
        "timestamp_le" => |a, _| times(a, O::is_le),
        "timestamp_gt" => |a, _| times(a, O::is_gt),
        "timestamp_ge" => |a, _| times(a, O::is_ge),
        "timestamp_eq_timestamptz" => |a, s| local_first(a, s, O::is_eq),
        "timestamp_ne_timestamptz" => |a, s| local_first(a, s, O::is_ne),
        "timestamp_lt_timestamptz" => |a, s| local_first(a, s, O::is_lt),
        "timestamp_le_timestamptz" => |a, s| local_first(a, s, O::is_le),
        "timestamp_gt_timestamptz" => |a, s| local_first(a, s, O::is_gt),
        "timestamp_ge_timestamptz" => |a, s| local_first(a, s, O::is_ge),
        "timestamptz_eq_timestamp" => |a, s| local_second(a, s, O::is_eq),
        "timestamptz_ne_timestamp" => |a, s| local_second(a, s, O::is_ne),
        "timestamptz_lt_timestamp" => |a, s| local_second(a, s, O::is_lt),
        "timestamptz_le_timestamp" => |a, s| local_second(a, s, O::is_le),
        "timestamptz_gt_timestamp" => |a, s| local_second(a, s, O::is_gt),
        "timestamptz_ge_timestamp" => |a, s| local_second(a, s, O::is_ge),
        // The casts.
        "int48" => |a, _| Ok(Value::Int8(i64::from(int4(a.first())?))),
        "int84" => |a, _| {
            let narrow = i32::try_from(int8(a.first())?).ok();
            narrow
                .map(Value::Int4)
                .ok_or_else(|| out_of_range("integer"))
        },
        "int4_bool" => |a, _| Ok(Value::Bool(int4(a.first())? != 0)),
        "bool_int4" => |a, _| Ok(Value::Int4(i32::from(boolean(a.first())?))),
        "booltext" => |a, _| {
            Ok(Value::Text(
                if boolean(a.first())? { "true" } else { "false" }.to_owned(),
            ))
        },
        "oidtoi8" => |a, _| Ok(Value::Int8(i64::from(oid(a.first())?))),
        "i8tooid" => |a, _| {
            let narrow = u32::try_from(int8(a.first())?).ok();
            narrow.map(Value::Oid).ok_or_else(|| out_of_range("OID"))
        },
        // char.c: chartoi4 widens a signed char, i4tochar refuses what doesn't fit one.
        "chartoi4" => |a, _| Ok(Value::Int4(i32::from(byte(a.first())?.cast_signed()))),
        "i4tochar" => |a, _| {
            let narrow = i8::try_from(int4(a.first())?).ok();
            narrow
                .map(|c| Value::Char(c.cast_unsigned()))
                .ok_or_else(|| out_of_range("\"char\""))
        },
        "char_text" => |a, _| Ok(Value::Text(char_text(byte(a.first())?))),
        "text_char" => |a, _| Ok(Value::Char(text_char(text(a.first())?))),
        // char_bpchar (varchar.c): the one byte, as it is.
        "char_bpchar" => |a, _| match byte(a.first())? {
            c if c.is_ascii() => Ok(Value::Text(char::from(c).to_string())),
            _ => Err(Error::unsupported(
                "a \"char\" that isn't ASCII as character",
            )),
        },
        "rtrim1" => |a, _| {
            let trimmed = text(a.first())?.trim_end_matches(' ');
            Ok(Value::Text(trimmed.to_owned()))
        },
        "bpchar" => |a, _| bpchar(text(a.first())?, int4(a.get(1))?, boolean(a.get(2))?),
        "current_schemas" => |a, s| current_schemas(boolean(a.first())?, s),
        "text_regclass" => |a, s| regclass(text(a.first())?, s),
        "timestamp_timestamptz" => |a, s| {
            datetime::to_timestamptz(timestamp(a.first())?, s.time_zone).map(Value::Timestamp)
        },
        "timestamptz_timestamp" => {
            |a, s| datetime::to_timestamp(timestamp(a.first())?, s.time_zone).map(Value::Timestamp)
        }
        "timestamp_scale" | "timestamptz_scale" => {
            |a, _| datetime::scale(timestamp(a.first())?, int4(a.get(1))?).map(Value::Timestamp)
        }
        _ => return None,
    };
    Some(body)
}

fn open_body(src: &str) -> Option<OpenBody> {
    let body: OpenBody = match src {
        "array_position" => |a| array_position(a.first(), a.get(1), None),
        "array_position_start" => |a| match a.get(2) {
            _ if matches!(a.first(), Some(None)) => Ok(None),
            Some(Some(start)) => array_position(a.first(), a.get(1), Some(int4(Some(start))?)),
            _ => Err(Error::new(
                sqlstate::NULL_VALUE_NOT_ALLOWED,
                "initial position must not be null".to_owned(),
            )),
        },
        _ => return None,
    };
    Some(body)
}

/// `array_position_common` (array_userfuncs.c): a NULL array has no position of anything.
fn array_position(
    array: Option<&Option<Value>>,
    sought: Option<&Option<Value>>,
    start: Option<i32>,
) -> Result<Option<Value>, Error> {
    let array = match array {
        Some(None) => return Ok(None),
        Some(Some(Value::Array(array))) => array,
        _ => return Err(mismatch()),
    };
    let sought = sought.ok_or_else(mismatch)?;
    Ok(array::position(array, sought.as_ref(), start)?.map(Value::Int4))
}

/// `current_schemas` (src/backend/utils/adt/name.c): the search path, its implicit schemas or not.
fn current_schemas(implicit: bool, session: &Session<'_>) -> Result<Value, Error> {
    let elements: Vec<Option<Value>> = session
        .path
        .fetch(implicit)
        .iter()
        .filter_map(|oid| session.relations.namespace_name(*oid))
        .map(|name| Some(Value::Text(name)))
        .collect();
    let dimensions = match i32::try_from(elements.len()) {
        Ok(0) => Vec::new(),
        Ok(length) => vec![array::Dimension { lower: 1, length }],
        Err(_) => return Err(Error::internal("a search path is too long")),
    };
    Ok(Value::Array(Array {
        element: type_oid::NAME,
        dimensions,
        elements,
    }))
}

/// `bpchar` (varchar.c:272): a value fitted to `char(n)`'s n characters, its typmod less VARHDRSZ.
/// What is cut off may only be spaces, unless the cast is explicit.
fn bpchar(source: &str, typmod: i32, explicit: bool) -> Result<Value, Error> {
    let Some(length) = typmod.checked_sub(4).and_then(|n| usize::try_from(n).ok()) else {
        return Ok(Value::Text(source.to_owned()));
    };
    let characters = source.chars().count();
    if characters > length {
        if !explicit && source.chars().skip(length).any(|c| c != ' ') {
            return Err(Error::new(
                sqlstate::STRING_DATA_RIGHT_TRUNCATION,
                format!("value too long for type character({length})"),
            ));
        }
        return Ok(Value::Text(source.chars().take(length).collect()));
    }
    let mut padded = source.to_owned();
    padded.extend(std::iter::repeat_n(' ', length.saturating_sub(characters)));
    Ok(Value::Text(padded))
}

/// A value under its column's type modifier (`bpchar_input`, varchar.c; a `char(n)`'s n characters).
pub fn fitted(type_oid: u32, typmod: i32, value: Value) -> Result<Value, Error> {
    match value {
        Value::Text(text) if type_oid == type_oid::BPCHAR => bpchar(&text, typmod, false),
        value => Ok(value),
    }
}

/// `charout` and `char_text` (char.c): nothing for the zero byte, `\ooo` for a byte past ASCII.
pub fn char_text(c: u8) -> String {
    match c {
        0 => String::new(),
        c if c.is_ascii() => char::from(c).to_string(),
        c => format!("\\{c:03o}"),
    }
}

/// `namein` (name.c): what of a name there is room for, cut at a character's end (`pg_mbcliplen`).
fn clipped(s: &str) -> &str {
    let mut end = s.len().min(NAMEDATALEN.saturating_sub(1));
    while !s.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    s.get(..end).unwrap_or_default()
}

/// `charin` and `text_char` (char.c): `\ooo`, else the first byte, else the zero byte.
pub fn text_char(s: &str) -> u8 {
    let octal = |b: u8| match b {
        b'0'..=b'7' => Some(b & 7),
        _ => None,
    };
    match s.as_bytes() {
        [b'\\', a, b, c] => match (octal(*a), octal(*b), octal(*c)) {
            (Some(a), Some(b), Some(c)) => (a << 6) | (b << 3) | c,
            _ => b'\\',
        },
        [first, ..] => *first,
        [] => 0,
    }
}

/// `pg_strtoint32_safe` and `pg_strtoint64_safe` (src/backend/utils/adt/numutils.c): spaces around a
/// sign and decimal digits, or 0x, 0o or 0b ones, with single underscores before digits.
fn integer(s: &str, name: &str) -> Result<i128, Error> {
    let invalid = || {
        Error::new(
            sqlstate::INVALID_TEXT_REPRESENTATION,
            format!("invalid input syntax for type {name}: \"{s}\""),
        )
    };
    let space = |c: char| matches!(c, ' ' | '\t'..='\r');
    let trimmed = s.trim_matches(space);
    let (negative, unsigned) = match trimmed.as_bytes() {
        [b'-', ..] => (true, trimmed.get(1..)),
        [b'+', ..] => (false, trimmed.get(1..)),
        _ => (false, Some(trimmed)),
    };
    let unsigned = unsigned.ok_or_else(invalid)?;
    let (radix, digits) = match unsigned.as_bytes() {
        [b'0', b'x' | b'X', ..] => (16, unsigned.get(2..)),
        [b'0', b'o' | b'O', ..] => (8, unsigned.get(2..)),
        [b'0', b'b' | b'B', ..] => (2, unsigned.get(2..)),
        _ => (10, Some(unsigned)),
    };
    let digits = digits.ok_or_else(invalid)?;
    // An underscore stands between digits (or after the radix prefix), never first in a decimal.
    let bare = digits.replace('_', "");
    if bare.is_empty()
        || digits.ends_with('_')
        || digits.contains("__")
        || (radix == 10 && digits.starts_with('_'))
        || !bare.chars().all(|c| c.is_digit(radix))
    {
        return Err(invalid());
    }
    let range = || {
        Error::new(
            sqlstate::NUMERIC_VALUE_OUT_OF_RANGE,
            format!("value \"{s}\" is out of range for type {name}"),
        )
    };
    let magnitude = i128::from_str_radix(&bare, radix).map_err(|_| range())?;
    if negative {
        magnitude.checked_neg().ok_or_else(range)
    } else {
        Ok(magnitude)
    }
}

fn integer_range(s: &str, name: &str) -> Error {
    Error::new(
        sqlstate::NUMERIC_VALUE_OUT_OF_RANGE,
        format!("value \"{s}\" is out of range for type {name}"),
    )
}

/// `parse_bool_with_len` (src/backend/utils/adt/bool.c): a case-insensitive prefix of true, false, yes
/// or no, on, off (by two letters at least), 1 or 0, with spaces around it.
pub(crate) fn parse_bool(s: &str) -> Option<bool> {
    let word = s
        .trim_matches(|c| matches!(c, ' ' | '\t'..='\r'))
        .to_ascii_lowercase();
    let prefix = |of: &str| !word.is_empty() && of.starts_with(&word);
    match word.as_str() {
        "1" => Some(true),
        "0" => Some(false),
        "on" => Some(true),
        "o" => None,
        w if "off".starts_with(w) && w.len() >= 2 => Some(false),
        _ if prefix("true") || prefix("yes") => Some(true),
        _ if prefix("false") || prefix("no") => Some(false),
        _ => None,
    }
}

/// `regclassin` (regproc.c:858): `-` or an OID, else a relation's name on the search path.
fn regclass(s: &str, session: &Session<'_>) -> Result<Value, Error> {
    if s == "-" {
        return Ok(Value::Oid(0));
    }
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        return oid_input(s);
    }
    let names = identifiers(s)?;
    let undefined = || {
        Error::new(
            sqlstate::UNDEFINED_TABLE,
            format!("relation \"{}\" does not exist", names.join(".")),
        )
    };
    let relations = session.relations;
    let found = match names.as_slice() {
        [name] => session
            .path
            .resolve(Kind::Relation, |ns| relations.relation(ns, name)),
        [schema, name] => relations
            .namespace(schema)
            .and_then(|ns| relations.relation(ns, name)),
        [_, _, _] => return Err(Error::unsupported("a relation in another database")),
        // makeRangeVarFromNameList (src/backend/catalog/namespace.c).
        _ => {
            return Err(Error::new(
                sqlstate::SYNTAX_ERROR,
                format!(
                    "improper relation name (too many dotted names): {}",
                    names.join(".")
                ),
            ));
        }
    };
    found.map(Value::Oid).ok_or_else(undefined)
}

/// `stringToQualifiedNameList` (regproc.c:1906): `SplitIdentifierString` by dots; 42602 when the string
/// is no name.
fn identifiers(s: &str) -> Result<Vec<String>, Error> {
    let invalid = || Error::new(sqlstate::INVALID_NAME, "invalid name syntax".to_owned());
    let space = |c: &char| matches!(c, ' ' | '\t'..='\r');
    let mut names = Vec::new();
    let mut rest = s.chars().peekable();
    loop {
        while rest.next_if(space).is_some() {}
        let mut name = String::new();
        if rest.next_if_eq(&'"').is_some() {
            loop {
                match rest.next() {
                    Some('"') if rest.next_if_eq(&'"').is_some() => name.push('"'),
                    Some('"') => break,
                    Some(c) => name.push(c),
                    None => return Err(invalid()),
                }
            }
        } else {
            while let Some(c) = rest.next_if(|c| *c != '.' && !space(c)) {
                name.push(c.to_ascii_lowercase());
            }
            if name.is_empty() {
                return Err(invalid());
            }
        }
        names.push(name);
        while rest.next_if(space).is_some() {}
        match rest.next() {
            Some('.') => {}
            None => return Ok(names),
            Some(_) => return Err(invalid()),
        }
    }
}

/// `oidin` (src/backend/utils/adt/oid.c) and `uint32in_subr` (numutils.c): C's strtoul of any base,
/// spaces around it; a negative number is taken as the 32 bits it is.
fn oid_input(s: &str) -> Result<Value, Error> {
    let invalid = || {
        Error::new(
            sqlstate::INVALID_TEXT_REPRESENTATION,
            format!("invalid input syntax for type oid: \"{s}\""),
        )
    };
    let trimmed = s.trim_matches(|c| matches!(c, ' ' | '\t'..='\r'));
    let (negative, unsigned) = match trimmed.as_bytes() {
        [b'-', ..] => (true, trimmed.get(1..)),
        [b'+', ..] => (false, trimmed.get(1..)),
        _ => (false, Some(trimmed)),
    };
    let unsigned = unsigned.ok_or_else(invalid)?;
    let (radix, digits) = match unsigned.as_bytes() {
        [b'0', b'x' | b'X', ..] => (16, unsigned.get(2..)),
        [b'0', _, ..] => (8, unsigned.get(1..)),
        _ => (10, Some(unsigned)),
    };
    let digits = digits.ok_or_else(invalid)?;
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return Err(invalid());
    }
    let magnitude = i64::from_str_radix(digits, radix).ok();
    let value = magnitude.and_then(|m| if negative { m.checked_neg() } else { Some(m) });
    let bits = value.and_then(|v| match u32::try_from(v) {
        Ok(v) => Some(v),
        Err(_) => i32::try_from(v).ok().map(i32::cast_unsigned),
    });
    bits.map(Value::Oid).ok_or_else(|| integer_range(s, "oid"))
}

/// A type's input function on a literal's text (`stringTypeDatum`), with no type modifier.
pub fn input(target: u32, s: &str, session: &Session<'_>) -> Result<Value, Error> {
    match element_type(target) {
        0 => plain_input(target, s, session),
        element => array::input(element, s, |s| plain_input(element, s, session)).map(Value::Array),
    }
}

/// [`input`] of a type that is no array.
fn plain_input(target: u32, s: &str, session: &Session<'_>) -> Result<Value, Error> {
    let function = type_by_oid(target)
        .and_then(|t| amphora_pg_catalog::proc_by_oid(t.input))
        .ok_or_else(|| Error::internal("a type has no input function"))?;
    match function.src {
        "int4in" => i32::try_from(integer(s, "integer")?)
            .map(Value::Int4)
            .map_err(|_| integer_range(s, "integer")),
        "int8in" => i64::try_from(integer(s, "bigint")?)
            .map(Value::Int8)
            .map_err(|_| integer_range(s, "bigint")),
        "textin" | "bpcharin" => Ok(Value::Text(s.to_owned())),
        "namein" => Ok(Value::Text(clipped(s).to_owned())),
        "charin" => Ok(Value::Char(text_char(s))),
        "boolin" => parse_bool(s).map(Value::Bool).ok_or_else(|| {
            Error::new(
                sqlstate::INVALID_TEXT_REPRESENTATION,
                format!("invalid input syntax for type boolean: \"{s}\""),
            )
        }),
        "oidin" => oid_input(s),
        "regclassin" => regclass(s, session),
        "timestamp_in" => datetime::timestamp_in(s).map(Value::Timestamp),
        "timestamptz_in" => datetime::timestamptz_in(s, session.time_zone).map(Value::Timestamp),
        _ => Err(Error::unsupported(&format!(
            "a literal of type {}",
            format_type(target)
        ))),
    }
}

/// A type's output function.
pub fn output(type_oid: u32, value: &Value, session: &Session<'_>) -> Result<String, Error> {
    match value {
        Value::Array(array) => {
            array::output(array, |value| plain_output(array.element, value, session))
        }
        value => plain_output(type_oid, value, session),
    }
}

/// [`output`] of a value that is no array.
fn plain_output(type_oid: u32, value: &Value, session: &Session<'_>) -> Result<String, Error> {
    Ok(match value {
        Value::Bool(b) => if *b { "t" } else { "f" }.to_owned(),
        Value::Char(c) => char_text(*c),
        Value::Int4(i) => i.to_string(),
        Value::Int8(i) => i.to_string(),
        Value::Text(s) | Value::Unknown(s) => s.clone(),
        Value::Oid(o) if type_oid == type_oid::REGCLASS => regclass_out(*o, session),
        Value::Oid(o) => o.to_string(),
        Value::Timestamp(t) if type_oid == type_oid::TIMESTAMPTZ => {
            datetime::timestamptz_out(*t, session.time_zone)?
        }
        Value::Timestamp(t) => datetime::timestamp_out(*t)?,
        Value::Numeric(_) => return Err(Error::unsupported("a numeric value as text")),
        Value::Array(_) => return Err(mismatch()),
    })
}

/// `regclassout` (regproc.c:951): the relation's name, qualified where the search path finds another
/// (or nothing) under it; the OID where no relation has it.
fn regclass_out(oid: u32, session: &Session<'_>) -> String {
    if oid == 0 {
        return "-".to_owned();
    }
    let relations = session.relations;
    let Some((namespace, name)) = relations.name(oid) else {
        return oid.to_string();
    };
    let visible = session
        .path
        .resolve(Kind::Relation, |ns| relations.relation(ns, &name))
        == Some(oid);
    if visible {
        return quote_identifier(&name);
    }
    match relations.namespace_name(namespace) {
        Some(schema) => format!("{}.{}", quote_identifier(&schema), quote_identifier(&name)),
        None => quote_identifier(&name),
    }
}

/// `quote_identifier` (src/backend/utils/adt/ruleutils.c), without its check for keywords: none of the
/// names it is asked for yet is one.
fn quote_identifier(name: &str) -> String {
    let start = |c: char| c.is_ascii_lowercase() || c == '_';
    let safe = name.starts_with(start) && name.chars().all(|c| start(c) || c.is_ascii_digit());
    if safe {
        name.to_owned()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

/// A step of [`eval`]'s walk.
enum Step {
    Enter(ExprId),
    Leave(ExprId),
    /// The argument of a BoolExpr just evaluated, and how many were before it; whether one was NULL.
    Bool {
        id: ExprId,
        done: usize,
        null: bool,
    },
}

/// An expression's value, as the executor computes it (execExprInterp.c): a strict function of a NULL
/// is NULL, AND and OR stop at the argument that decides them. `params` are `$n`'s values. A column
/// has no value here.
pub fn eval(
    exprs: &Exprs,
    root: ExprId,
    session: &Session<'_>,
    params: &[Option<Value>],
) -> Result<Option<Value>, Error> {
    let mut steps = vec![Step::Enter(root)];
    let mut values: Vec<Option<Value>> = Vec::new();
    while let Some(step) = steps.pop() {
        match step {
            Step::Enter(id) => match &exprs.get(id)?.kind {
                ExprKind::Const(value) => values.push(value.clone()),
                ExprKind::Param(n) => {
                    let value = usize::try_from(*n)
                        .ok()
                        .and_then(|n| n.checked_sub(1))
                        .and_then(|i| params.get(i))
                        .ok_or_else(|| {
                            Error::new(
                                sqlstate::UNDEFINED_OBJECT,
                                format!("no value found for parameter {n}"),
                            )
                        })?;
                    values.push(value.clone());
                }
                ExprKind::Op { args, .. } | ExprKind::Func { args, .. } => {
                    steps.push(Step::Leave(id));
                    steps.extend(args.iter().rev().map(|a| Step::Enter(*a)));
                }
                ExprKind::Relabel { arg, .. }
                | ExprKind::ViaIo { arg, .. }
                | ExprKind::NullTest { arg, .. } => {
                    steps.push(Step::Leave(id));
                    steps.push(Step::Enter(*arg));
                }
                ExprKind::Bool { args, .. } => {
                    let first = args
                        .first()
                        .ok_or_else(|| Error::internal("a boolean expression is empty"))?;
                    steps.push(Step::Bool {
                        id,
                        done: 0,
                        null: false,
                    });
                    steps.push(Step::Enter(*first));
                }
                ExprKind::Var { .. } => return Err(Error::unsupported("a column outside a row")),
                ExprKind::Agg { .. } => {
                    return Err(Error::unsupported("an aggregate outside its group"));
                }
                ExprKind::CurrentTimestamp => match session.now {
                    Some(t) => values.push(Some(Value::Timestamp(t))),
                    None => {
                        return Err(Error::unsupported(
                            "CURRENT_TIMESTAMP outside a transaction",
                        ));
                    }
                },
            },
            Step::Leave(id) => leave(exprs, id, session, &mut values)?,
            Step::Bool { id, done, null } => {
                let ExprKind::Bool { op, args } = &exprs.get(id)?.kind else {
                    return Err(Error::internal("a boolean expression is missing"));
                };
                let value = match values.pop().flatten() {
                    None => None,
                    Some(v) => Some(boolean(Some(&v))?),
                };
                let decides = match op {
                    BoolOp::And => Some(false),
                    BoolOp::Or => Some(true),
                    BoolOp::Not => {
                        values.push(value.map(|b| Value::Bool(!b)));
                        continue;
                    }
                };
                let null = null || value.is_none();
                if value.is_some() && value == decides {
                    values.push(value.map(Value::Bool));
                } else if let Some(next) = args.get(done.saturating_add(1)) {
                    steps.push(Step::Bool {
                        id,
                        done: done.saturating_add(1),
                        null,
                    });
                    steps.push(Step::Enter(*next));
                } else if null {
                    values.push(None);
                } else {
                    values.push(value.map(Value::Bool));
                }
            }
        }
    }
    values
        .pop()
        .ok_or_else(|| Error::internal("an expression has no value"))
}

/// The value of a node whose arguments' values are the last ones.
fn leave(
    exprs: &Exprs,
    id: ExprId,
    session: &Session<'_>,
    values: &mut Vec<Option<Value>>,
) -> Result<(), Error> {
    let e = exprs.get(id)?;
    let value = match &e.kind {
        ExprKind::Op { function, args, .. } | ExprKind::Func { function, args, .. } => {
            let at = values.len().saturating_sub(args.len());
            let args = values.split_off(at);
            call(function, &args, session)?
        }
        ExprKind::Relabel { .. } => values.pop().flatten().map(|v| relabel(v, e.type_oid)),
        ExprKind::ViaIo { arg, .. } => match values.pop().flatten() {
            None => None,
            Some(v) => {
                let text = output(exprs.get(*arg)?.type_oid, &v, session)?;
                Some(input(e.type_oid, &text, session)?)
            }
        },
        ExprKind::NullTest { not, .. } => {
            Some(Value::Bool(values.pop().flatten().is_some() == *not))
        }
        _ => return Err(Error::internal("an expression has no value")),
    };
    values.push(value);
    Ok(())
}

/// A built-in function's value over these arguments: NULL over a NULL where it is strict
/// (src/backend/utils/fmgr/README). `0A000` where its body isn't here.
pub fn call(
    function: &Proc,
    args: &[Option<Value>],
    session: &Session<'_>,
) -> Result<Option<Value>, Error> {
    if let (false, Some(body)) = (function.strict, open_body(function.src)) {
        return body(args);
    }
    let body = body(function.src)
        .ok_or_else(|| Error::unsupported(&format!("function {}", function.name)))?;
    let values: Option<Vec<Value>> = args.iter().cloned().collect();
    values.map(|values| body(&values, session)).transpose()
}

/// A value under a binary-compatible type's name: an int4 and an oid are the same 32 bits.
fn relabel(value: Value, target: u32) -> Value {
    match value {
        Value::Int4(i) if matches!(target, type_oid::OID | type_oid::REGCLASS) => {
            Value::Oid(i.cast_unsigned())
        }
        Value::Oid(o) if target == type_oid::INT4 => Value::Int4(o.cast_signed()),
        value => value,
    }
}
