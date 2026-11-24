//! The binary formats (doc/src/sgml/protocol.sgml, "Formats and Format Codes"): a type's `typreceive`
//! on a parameter's bytes and its `typsend` on a value, for the types [`crate::func`] has values of.

use amphora_pg_catalog::data::PG_TYPE;
use amphora_pg_catalog::{Datum, Proc, proc_by_oid};

use crate::func::Value;
use crate::{Error, datetime, format_type, sqlstate};

/// The bytes a name has room for, its terminator with them (src/include/pg_config_manual.h:39).
pub const NAMEDATALEN: usize = 64;

/// `getTypeBinaryInputInfo` and `getTypeBinaryOutputInfo` (src/backend/utils/cache/lsyscache.c): the
/// function a type's row of `pg_type` names for a direction, 42883 where it names none.
fn function(type_oid: u32, column: &str, direction: &str) -> Result<&'static Proc, Error> {
    let row = PG_TYPE.column("oid").and_then(|at| {
        PG_TYPE
            .rows
            .iter()
            .find(|row| row.get(at) == Some(&Datum::Oid(type_oid)))
    });
    let named = PG_TYPE
        .column(column)
        .and_then(|at| match row.and_then(|row| row.get(at)) {
            Some(Datum::Oid(oid)) => Some(*oid),
            _ => None,
        });
    match named {
        Some(oid) if oid != 0 => proc_by_oid(oid)
            .ok_or_else(|| Error::internal("a type's binary function is missing from pg_proc")),
        _ => Err(Error::new(
            sqlstate::UNDEFINED_FUNCTION,
            format!(
                "no binary {direction} function available for type {}",
                format_type(type_oid)
            ),
        )),
    }
}

fn insufficient() -> Error {
    Error::new(
        sqlstate::PROTOCOL_VIOLATION,
        "insufficient data left in message".to_owned(),
    )
}

/// The first `N` bytes of a message's part (`pq_getmsgint`, src/backend/libpq/pqformat.c).
fn head<const N: usize>(bytes: &[u8]) -> Result<[u8; N], Error> {
    bytes
        .get(..N)
        .and_then(|head| head.try_into().ok())
        .ok_or_else(insufficient)
}

/// `pg_client_to_server` (src/backend/utils/mb/mbutils.c) from UTF8: the text these bytes are, 22021
/// where they aren't any, the zero byte with them, as `report_invalid_encoding` shows it.
pub fn text(bytes: &[u8]) -> Result<&str, Error> {
    let zero = bytes.iter().position(|b| *b == 0);
    let (valid, whole) = match std::str::from_utf8(bytes) {
        Ok(text) => (bytes.len(), Some(text)),
        Err(error) => (error.valid_up_to(), None),
    };
    let at = match (zero, whole) {
        (None, Some(text)) => return Ok(text),
        (Some(zero), _) => zero.min(valid),
        (None, None) => valid,
    };
    let rest = bytes.get(at..).unwrap_or_default();
    // pg_utf_mblen: what the first byte says the character takes.
    let length = match rest.first() {
        Some(b) if b & 0xe0 == 0xc0 => 2,
        Some(b) if b & 0xf0 == 0xe0 => 3,
        Some(b) if b & 0xf8 == 0xf0 => 4,
        _ => 1,
    };
    let shown: Vec<String> = rest
        .iter()
        .take(length)
        .map(|b| format!("0x{b:02x}"))
        .collect();
    Err(Error::new(
        sqlstate::CHARACTER_NOT_IN_REPERTOIRE,
        format!(
            "invalid byte sequence for encoding \"UTF8\": {}",
            shown.join(" ")
        ),
    ))
}

/// Whether a type's values can be received, a NULL's as well.
pub fn receivable(type_oid: u32) -> Result<(), Error> {
    function(type_oid, "typreceive", "input").map(|_| ())
}

/// A type's `typreceive` on a value's bytes, and how many of them it read. A type modifier is the
/// statement's to apply: a Bind's parameter has none.
pub fn receive(type_oid: u32, bytes: &[u8]) -> Result<(Value, usize), Error> {
    let function = function(type_oid, "typreceive", "input")?;
    Ok(match function.src {
        "boolrecv" => (Value::Bool(head::<1>(bytes)? != [0]), 1),
        "charrecv" => {
            let [byte] = head(bytes)?;
            (Value::Char(byte), 1)
        }
        "int4recv" => (Value::Int4(i32::from_be_bytes(head(bytes)?)), 4),
        "int8recv" => (Value::Int8(i64::from_be_bytes(head(bytes)?)), 8),
        "oidrecv" | "regclassrecv" => (Value::Oid(u32::from_be_bytes(head(bytes)?)), 4),
        "textrecv" | "bpcharrecv" => (Value::Text(text(bytes)?.to_owned()), bytes.len()),
        "namerecv" => {
            if bytes.len() >= NAMEDATALEN {
                return Err(Error::new(
                    sqlstate::NAME_TOO_LONG,
                    "identifier too long".to_owned(),
                ));
            }
            (Value::Text(text(bytes)?.to_owned()), bytes.len())
        }
        "timestamp_recv" | "timestamptz_recv" => {
            let t = datetime::received(i64::from_be_bytes(head(bytes)?))?;
            (Value::Timestamp(t), 8)
        }
        _ => return Err(unbuilt(type_oid)),
    })
}

/// A type's `typsend`.
pub fn send(type_oid: u32, value: &Value) -> Result<Vec<u8>, Error> {
    let function = function(type_oid, "typsend", "output")?;
    Ok(match (function.src, value) {
        ("boolsend", Value::Bool(b)) => vec![u8::from(*b)],
        ("charsend", Value::Char(c)) => vec![*c],
        ("int4send", Value::Int4(i)) => i.to_be_bytes().to_vec(),
        ("int8send", Value::Int8(i)) => i.to_be_bytes().to_vec(),
        ("oidsend" | "regclasssend", Value::Oid(o)) => o.to_be_bytes().to_vec(),
        ("textsend" | "bpcharsend" | "namesend", Value::Text(s)) => s.as_bytes().to_vec(),
        ("timestamp_send" | "timestamptz_send", Value::Timestamp(t)) => t.to_be_bytes().to_vec(),
        _ => return Err(unbuilt(type_oid)),
    })
}

fn unbuilt(type_oid: u32) -> Error {
    Error::unsupported(&format!(
        "the binary format of type {}",
        format_type(type_oid)
    ))
}
