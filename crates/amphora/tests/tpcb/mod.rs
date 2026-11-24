//! What a PostgreSQL 19 server answered pgbench's statements (scripts/postgres-tpcb-answers.py), and
//! what of an answer a client can tell.

/// A timestamp's text as 19 writes it: to the microsecond, less its trailing zeros.
fn is_timestamp(text: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(text) else {
        return false;
    };
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let shape: String = whole
        .chars()
        .map(|c| if c.is_ascii_digit() { '0' } else { c })
        .collect();
    shape == "0000-00-00 00:00:00"
        && fraction.len() <= 6
        && fraction.bytes().all(|b| b.is_ascii_digit())
        && !text.ends_with('.')
        && !fraction.ends_with('0')
}

fn be16(body: &[u8], at: usize) -> u16 {
    u16::from_be_bytes(body[at..at + 2].try_into().unwrap())
}

fn be32(body: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(body[at..at + 4].try_into().unwrap())
}

/// What a client can tell of a statement's answer, message by message: RowDescription without the
/// table's OID but whether it has one, a refusal or a notice by its severity, SQLSTATE, message and
/// position (S, V, C, M, P: its DETAIL and HINT aren't made yet, and the rest is the server's own source
/// file, line and routine), a CURRENT_TIMESTAMP by its shape, and everything else as it is.
pub fn told(sql: &str, messages: &[(u8, Vec<u8>)]) -> Vec<String> {
    let timestamps: &[usize] = if sql.contains("mtime FROM") {
        &[4]
    } else {
        &[]
    };
    messages
        .iter()
        .map(|(kind, body)| message(*kind, body, timestamps))
        .collect()
}

/// A message as a client of the extended protocol can tell it: as [`told`] does, but a row's values by
/// their bytes, whatever their format, and a refusal's context with it.
pub fn exactly(kind: u8, body: &[u8]) -> String {
    match kind {
        b'E' | b'N' => refused(kind, body, b"SVCMWP"),
        b'D' => format!("D {body:02x?}"),
        _ => message(kind, body, &[]),
    }
}

fn refused(kind: u8, body: &[u8], told: &[u8]) -> String {
    let fields: Vec<String> = body
        .split(|b| *b == 0)
        .filter(|f| f.first().is_some_and(|tag| told.contains(tag)))
        .map(|f| String::from_utf8_lossy(f).into_owned())
        .collect();
    format!("{} {fields:?}", char::from(kind))
}

fn message(kind: u8, body: &[u8], timestamps: &[usize]) -> String {
    match kind {
        b'T' => {
            let mut fields = Vec::new();
            let mut at = 2;
            for _ in 0..be16(body, 0) {
                let end = at + body[at..].iter().position(|b| *b == 0).unwrap();
                let name = String::from_utf8(body[at..end].to_vec()).unwrap();
                at = end + 1;
                let (table, attnum) = (be32(body, at) != 0, be16(body, at + 4));
                let (type_oid, typlen) = (be32(body, at + 6), be16(body, at + 10).cast_signed());
                let (typmod, format) = (be32(body, at + 12).cast_signed(), be16(body, at + 16));
                fields.push(format!(
                    "{name}: table {table} column {attnum}, type {type_oid}, {typlen}, {typmod}, format {format}"
                ));
                at += 18;
            }
            format!("T {fields:?}")
        }
        b'D' => {
            let mut values = Vec::new();
            let mut at = 2;
            for column in 0..usize::from(be16(body, 0)) {
                let length = be32(body, at).cast_signed();
                at += 4;
                let Ok(length) = usize::try_from(length) else {
                    values.push("NULL".to_owned());
                    continue;
                };
                let value = &body[at..at + length];
                at += length;
                values.push(if timestamps.contains(&column) && is_timestamp(value) {
                    "a timestamp".to_owned()
                } else {
                    String::from_utf8_lossy(value).into_owned()
                });
            }
            format!("D {values:?}")
        }
        b'C' => format!("C {}", String::from_utf8_lossy(body)),
        b'E' | b'N' => refused(kind, body, b"SVCMP"),
        b'Z' => format!("Z {}", String::from_utf8_lossy(body)),
        _ => format!("{} {body:02x?}", char::from(kind)),
    }
}

/// A session's name, its statement, and what a 19 server answered it: `@ session sql`, then each
/// message's type and hex body (scripts/postgres-tpcb-answers.py).
pub type Step = (String, String, Vec<(u8, Vec<u8>)>);

pub fn answers(text: &str) -> Vec<Step> {
    let mut steps: Vec<Step> = Vec::new();
    for line in text.lines() {
        if let Some(step) = line.strip_prefix("@ ") {
            let (session, sql) = step.split_once(' ').unwrap();
            steps.push((session.to_owned(), sql.to_owned(), Vec::new()));
            continue;
        }
        let (kind, hex) = line.split_once(' ').unwrap_or((line, ""));
        let body = hex
            .as_bytes()
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        let kind = kind.bytes().next().unwrap();
        steps.last_mut().unwrap().2.push((kind, body));
    }
    steps
}
