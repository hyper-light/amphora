//! Arrays: their text and binary forms and the key the engine stores one under, over the functions of
//! their elements' type (REL_19_0 src/backend/utils/adt/arrayfuncs.c and arrayutils.c;
//! docs/design/postgres.md section 5). Every loop is flat (CLAUDE.md section 11).

use std::iter::Peekable;
use std::str::Chars;

use amphora_pg_catalog::data::type_oid;

use crate::func::Value;
use crate::{Error, format_type, sqlstate};

/// `MAXDIM` (src/include/utils/array.h:75).
const MAX_DIMENSIONS: usize = 6;

/// `MaxArraySize` (array.h:82): `MaxAllocSize / sizeof(Datum)`, 0x3fffffff bytes of 8 each.
const MAX_SIZE: i32 = 0x3fff_ffff / 8;

/// One dimension's lower bound (`ARR_LBOUND`) and length (`ARR_DIMS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dimension {
    pub lower: i32,
    pub length: i32,
}

/// An array (`ArrayType`): its elements' type, its dimensions (none for an empty one) and its elements, the
/// last dimension's changing first.
#[derive(Debug, Clone, PartialEq)]
pub struct Array {
    pub element: u32,
    pub dimensions: Vec<Dimension>,
    pub elements: Vec<Option<Value>>,
}

/// `scanner_isspace` (src/backend/parser/scansup.c).
fn space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')
}

fn too_many_dimensions() -> Error {
    Error::new(
        sqlstate::PROGRAM_LIMIT_EXCEEDED,
        format!("number of array dimensions exceeds the maximum allowed ({MAX_DIMENSIONS})"),
    )
}

fn too_large() -> Error {
    Error::new(
        sqlstate::PROGRAM_LIMIT_EXCEEDED,
        format!("array size exceeds the maximum allowed ({MAX_SIZE})"),
    )
}

/// `ArrayToken`: what [`Reader::token`] reads.
enum Token {
    Open,
    Close,
    Delimiter,
    Element(String),
    Null,
}

/// An array's text, read from left to right.
struct Reader<'a> {
    text: &'a str,
    rest: Peekable<Chars<'a>>,
}

impl Reader<'_> {
    fn malformed(&self) -> Error {
        Error::new(
            sqlstate::INVALID_TEXT_REPRESENTATION,
            format!("malformed array literal: \"{}\"", self.text),
        )
    }

    fn spaces(&mut self) {
        while self.rest.next_if(|c| space(*c)).is_some() {}
    }

    /// `ReadDimensionInt` (arrayfuncs.c:523): none where there are no digits.
    fn bound(&mut self) -> Result<Option<i32>, Error> {
        let mut digits = String::new();
        digits.extend(self.rest.next_if(|c| matches!(c, '-' | '+')));
        while let Some(digit) = self.rest.next_if(char::is_ascii_digit) {
            digits.push(digit);
        }
        if !digits.ends_with(|c: char| c.is_ascii_digit()) {
            return Ok(None);
        }
        digits.parse().map(Some).map_err(|_| {
            Error::new(
                sqlstate::ARRAY_SUBSCRIPT_ERROR,
                "array bound is out of integer range".to_owned(),
            )
        })
    }

    /// `ReadArrayDimensions` (arrayfuncs.c:406): the `[lower:upper]`s before an array's braces.
    fn dimensions(&mut self) -> Result<Vec<Dimension>, Error> {
        let mut dimensions = Vec::new();
        loop {
            self.spaces();
            if self.rest.next_if_eq(&'[').is_none() {
                return Ok(dimensions);
            }
            if dimensions.len() >= MAX_DIMENSIONS {
                return Err(too_many_dimensions());
            }
            let first = self.bound()?.ok_or_else(|| self.malformed())?;
            let (lower, upper) = if self.rest.next_if_eq(&':').is_some() {
                (first, self.bound()?.ok_or_else(|| self.malformed())?)
            } else {
                (1, first)
            };
            if self.rest.next_if_eq(&']').is_none() {
                return Err(self.malformed());
            }
            if upper < lower {
                return Err(Error::new(
                    sqlstate::ARRAY_SUBSCRIPT_ERROR,
                    "upper bound cannot be less than lower bound".to_owned(),
                ));
            }
            if upper == i32::MAX {
                return Err(Error::new(
                    sqlstate::PROGRAM_LIMIT_EXCEEDED,
                    format!("array upper bound is too large: {upper}"),
                ));
            }
            let length = upper
                .checked_sub(lower)
                .and_then(|length| length.checked_add(1))
                .ok_or_else(too_large)?;
            dimensions.push(Dimension { lower, length });
        }
    }

    /// `ReadArrayToken` (arrayfuncs.c:800).
    fn token(&mut self) -> Result<Token, Error> {
        self.spaces();
        match self.rest.peek() {
            None => return Err(self.malformed()),
            Some('{') => {
                self.rest.next();
                return Ok(Token::Open);
            }
            Some('}') => {
                self.rest.next();
                return Ok(Token::Close);
            }
            Some(',') => {
                self.rest.next();
                return Ok(Token::Delimiter);
            }
            Some('"') => {
                self.rest.next();
                return self.quoted();
            }
            Some(_) => {}
        }
        let mut element = String::new();
        // What is known not to be trailing spaces.
        let mut kept = 0;
        let mut escaped = false;
        loop {
            match self.rest.peek() {
                None | Some('{' | '"') => return Err(self.malformed()),
                Some('\\') => {
                    self.rest.next();
                    element.push(self.rest.next().ok_or_else(|| self.malformed())?);
                    kept = element.len();
                    escaped = true;
                }
                Some(',' | '}') => {
                    element.truncate(kept);
                    if !escaped && element.eq_ignore_ascii_case("NULL") {
                        return Ok(Token::Null);
                    }
                    return Ok(Token::Element(element));
                }
                Some(c) => {
                    let c = *c;
                    element.push(c);
                    if !space(c) {
                        kept = element.len();
                    }
                    self.rest.next();
                }
            }
        }
    }

    /// The rest of an element in double quotes.
    fn quoted(&mut self) -> Result<Token, Error> {
        let mut element = String::new();
        loop {
            match self.rest.next().ok_or_else(|| self.malformed())? {
                '\\' => element.push(self.rest.next().ok_or_else(|| self.malformed())?),
                '"' => break,
                c => element.push(c),
            }
        }
        self.spaces();
        match self.rest.peek() {
            Some(',' | '}' | '{') => Ok(Token::Element(element)),
            _ => Err(self.malformed()),
        }
    }
}

/// `array_in` (arrayfuncs.c:181) and `ReadArrayStr` (:583), over the input function of the elements' type,
/// whose delimiter is a comma.
pub fn input(
    element: u32,
    text: &str,
    read: impl Fn(&str) -> Result<Value, Error>,
) -> Result<Array, Error> {
    let mut reader = Reader {
        text,
        rest: text.chars().peekable(),
    };
    let given = reader.dimensions()?;
    if !given.is_empty() {
        if reader.rest.next_if_eq(&'=').is_none() {
            return Err(reader.malformed());
        }
        reader.spaces();
    }
    if reader.rest.peek() != Some(&'{') {
        return Err(reader.malformed());
    }
    // Each level's length, once known, and how many elements of it have been read.
    let mut lengths = [None; MAX_DIMENSIONS];
    for (length, dimension) in lengths.iter_mut().zip(&given) {
        *length = Some(dimension.length);
    }
    let mut counts = [0i32; MAX_DIMENSIONS];
    let mut depth = given.len();
    let mut frozen = !given.is_empty();
    let mut level: usize = 0;
    let mut expects_delimiter = false;
    let mut elements = Vec::new();
    loop {
        match reader.token()? {
            Token::Open => {
                if expects_delimiter {
                    return Err(reader.malformed());
                }
                *counts.get_mut(level).ok_or_else(too_many_dimensions)? = 0;
                level = level.saturating_add(1);
                if level > depth {
                    if frozen {
                        return Err(reader.malformed());
                    }
                    depth = level;
                }
            }
            Token::Close => {
                level = level.checked_sub(1).ok_or_else(|| reader.malformed())?;
                let count = counts.get(level).copied().unwrap_or_default();
                if count > 0 && !expects_delimiter {
                    return Err(reader.malformed());
                }
                if let Some(outer) = level.checked_sub(1).and_then(|at| counts.get_mut(at)) {
                    *outer = outer.saturating_add(1);
                }
                let length = lengths
                    .get_mut(level)
                    .ok_or_else(too_many_dimensions)?
                    .get_or_insert(count);
                if *length != count {
                    return Err(reader.malformed());
                }
                expects_delimiter = true;
            }
            Token::Delimiter => {
                if !expects_delimiter {
                    return Err(reader.malformed());
                }
                expects_delimiter = false;
            }
            token @ (Token::Element(_) | Token::Null) => {
                if expects_delimiter {
                    return Err(reader.malformed());
                }
                elements.push(match token {
                    Token::Element(text) => Some(read(&text)?),
                    _ => None,
                });
                frozen = true;
                if level != depth {
                    return Err(reader.malformed());
                }
                if let Some(count) = level.checked_sub(1).and_then(|at| counts.get_mut(at)) {
                    *count = count.saturating_add(1);
                }
                expects_delimiter = true;
            }
        }
        if level == 0 {
            break;
        }
    }
    reader.spaces();
    if reader.rest.next().is_some() {
        return Err(reader.malformed());
    }
    let mut dimensions = Vec::new();
    if !elements.is_empty() {
        for (at, length) in lengths.iter().take(depth).enumerate() {
            dimensions.push(Dimension {
                lower: given.get(at).map_or(1, |dimension| dimension.lower),
                length: length.unwrap_or_default(),
            });
        }
    }
    Ok(Array {
        element,
        dimensions,
        elements,
    })
}

/// `array_out` (arrayfuncs.c:1021), over the output function of the elements' type, whose delimiter is a comma.
pub fn output(
    array: &Array,
    write: impl Fn(&Value) -> Result<String, Error>,
) -> Result<String, Error> {
    let mut text = String::new();
    if array.dimensions.iter().any(|d| d.lower != 1) {
        for d in &array.dimensions {
            let upper = i64::from(d.lower)
                .saturating_add(i64::from(d.length))
                .saturating_sub(1);
            text.push_str(&format!("[{}:{upper}]", d.lower));
        }
        text.push('=');
    }
    text.push('{');
    if array.elements.is_empty() {
        text.push('}');
        return Ok(text);
    }
    let inner = array.dimensions.len().saturating_sub(1);
    let mut index = vec![0i32; array.dimensions.len()];
    // The dimension the last element's delimiter was in.
    let mut left_at = 0;
    for element in &array.elements {
        for _ in left_at..inner {
            text.push('{');
        }
        match element {
            None => text.push_str("NULL"),
            Some(value) => {
                let value = write(value)?;
                let quotes = value.is_empty()
                    || value.eq_ignore_ascii_case("NULL")
                    || value.contains(|c| matches!(c, '"' | '\\' | '{' | '}' | ',') || space(c));
                if quotes {
                    text.push('"');
                    for c in value.chars() {
                        if matches!(c, '"' | '\\') {
                            text.push('\\');
                        }
                        text.push(c);
                    }
                    text.push('"');
                } else {
                    text.push_str(&value);
                }
            }
        }
        for (at, (i, d)) in index.iter_mut().zip(&array.dimensions).enumerate().rev() {
            *i = i.saturating_add(1);
            if *i < d.length {
                text.push(',');
                left_at = at;
                break;
            }
            *i = 0;
            text.push('}');
        }
    }
    Ok(text)
}

fn insufficient() -> Error {
    Error::new(
        sqlstate::PROTOCOL_VIOLATION,
        "insufficient data left in message".to_owned(),
    )
}

/// `pq_getmsgint` (src/backend/libpq/pqformat.c) of four bytes: what is left of a message's part after them.
fn word(bytes: &[u8]) -> Result<(i32, &[u8]), Error> {
    let (head, rest) = bytes.split_first_chunk::<4>().ok_or_else(insufficient)?;
    Ok((i32::from_be_bytes(*head), rest))
}

/// `array_send` (arrayfuncs.c:1553), over the send function of the elements' type.
pub fn send(
    array: &Array,
    write: impl Fn(&Value) -> Result<Vec<u8>, Error>,
) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    let dimensions = u32::try_from(array.dimensions.len()).map_err(|_| too_many_dimensions())?;
    bytes.extend_from_slice(&dimensions.to_be_bytes());
    let nulls = i32::from(array.elements.iter().any(Option::is_none));
    bytes.extend_from_slice(&nulls.to_be_bytes());
    bytes.extend_from_slice(&array.element.to_be_bytes());
    for d in &array.dimensions {
        bytes.extend_from_slice(&d.length.to_be_bytes());
        bytes.extend_from_slice(&d.lower.to_be_bytes());
    }
    for element in &array.elements {
        match element {
            None => bytes.extend_from_slice(&(-1i32).to_be_bytes()),
            Some(value) => {
                let value = write(value)?;
                let length = i32::try_from(value.len()).map_err(|_| too_large())?;
                bytes.extend_from_slice(&length.to_be_bytes());
                bytes.extend_from_slice(&value);
            }
        }
    }
    Ok(bytes)
}

/// `array_recv` (arrayfuncs.c:1275), over the receive function of the elements' type: the array and how many
/// of the bytes it took.
pub fn receive(
    element: u32,
    bytes: &[u8],
    read: impl Fn(&[u8]) -> Result<(Value, usize), Error>,
) -> Result<(Array, usize), Error> {
    let invalid =
        |message: String| Error::new(sqlstate::INVALID_BINARY_REPRESENTATION, message);
    let (count, rest) = word(bytes)?;
    if count < 0 {
        return Err(invalid(format!("invalid number of dimensions: {count}")));
    }
    let count = usize::try_from(count).map_err(|_| too_many_dimensions())?;
    if count > MAX_DIMENSIONS {
        return Err(Error::new(
            sqlstate::PROGRAM_LIMIT_EXCEEDED,
            format!(
                "number of array dimensions ({count}) exceeds the maximum allowed ({MAX_DIMENSIONS})"
            ),
        ));
    }
    let (flags, rest) = word(rest)?;
    if !matches!(flags, 0 | 1) {
        return Err(invalid("invalid array flags".to_owned()));
    }
    let (sent, mut rest) = word(rest)?;
    let sent = sent.cast_unsigned();
    if sent != element && sent < FIRST_GENBKI_OBJECT_ID && element < FIRST_GENBKI_OBJECT_ID {
        return Err(Error::new(
            sqlstate::DATATYPE_MISMATCH,
            format!(
                "binary data has array element type {sent} ({}) instead of expected {element} ({})",
                format_type(sent),
                format_type(element)
            ),
        ));
    }
    let mut dimensions = Vec::with_capacity(count);
    for _ in 0..count {
        let (length, after) = word(rest)?;
        let (lower, after) = word(after)?;
        dimensions.push(Dimension { lower, length });
        rest = after;
    }
    // ArrayGetNItems and ArrayCheckBounds (arrayutils.c).
    let mut items: i32 = if dimensions.is_empty() { 0 } else { 1 };
    for d in &dimensions {
        if d.length < 0 {
            return Err(too_large());
        }
        items = items.checked_mul(d.length).ok_or_else(too_large)?;
    }
    if items > MAX_SIZE {
        return Err(too_large());
    }
    for d in &dimensions {
        if d.lower.checked_add(d.length).is_none() {
            return Err(Error::new(
                sqlstate::PROGRAM_LIMIT_EXCEEDED,
                format!("array lower bound is too large: {}", d.lower),
            ));
        }
    }
    let mut elements = Vec::new();
    for at in 0..items {
        let (length, after) = word(rest)?;
        if length == -1 {
            elements.push(None);
            rest = after;
            continue;
        }
        let (value, after) = usize::try_from(length)
            .ok()
            .and_then(|length| after.split_at_checked(length))
            .ok_or_else(|| invalid("insufficient data left in message".to_owned()))?;
        let (read, taken) = read(value)?;
        if taken != value.len() {
            return Err(invalid(format!(
                "improper binary format in array element {}",
                at.saturating_add(1)
            )));
        }
        elements.push(Some(read));
        rest = after;
    }
    if elements.is_empty() {
        dimensions.clear();
    }
    let taken = bytes.len().saturating_sub(rest.len());
    Ok((
        Array {
            element,
            dimensions,
            elements,
        },
        taken,
    ))
}

/// `FirstGenbkiObjectId` (src/include/access/transam.h): the first OID genbki.pl gives.
const FIRST_GENBKI_OBJECT_ID: u32 = 10_000;

/// What follows an element in a key: nothing more, a value, or a NULL, which `array_cmp` orders last.
const END: u8 = 0;
const VALUE: u8 = 1;
const NULL: u8 = 2;

/// The array's key (section 5): its elements' keys in order, then its dimensions and lower bounds, so the
/// engine's order of keys is `array_cmp`'s (arrayfuncs.c). None for elements no key is made of yet.
pub fn key(array: &Array) -> Option<Vec<u8>> {
    let mut key = Vec::new();
    for element in &array.elements {
        let Some(value) = element else {
            key.push(NULL);
            continue;
        };
        key.push(VALUE);
        match value {
            Value::Text(text) if array.element != type_oid::BPCHAR => {
                if text.contains('\0') {
                    return None;
                }
                key.extend_from_slice(text.as_bytes());
                key.push(END);
            }
            Value::Char(c) => key.push(*c),
            value => key.extend_from_slice(&integer(value)?.to_be_bytes()),
        }
    }
    key.push(END);
    key.push(u8::try_from(array.dimensions.len()).ok()?);
    for d in &array.dimensions {
        key.extend_from_slice(&ordered(i64::from(d.length)).to_be_bytes());
    }
    for d in &array.dimensions {
        key.extend_from_slice(&ordered(i64::from(d.lower)).to_be_bytes());
    }
    Some(key)
}

/// A value the engine stores as an integer.
fn integer(value: &Value) -> Option<u64> {
    let i = match value {
        Value::Bool(b) => i64::from(*b),
        Value::Int4(i) => i64::from(*i),
        Value::Int8(i) | Value::Timestamp(i) => *i,
        Value::Oid(o) => i64::from(*o),
        _ => return None,
    };
    Some(ordered(i))
}

/// An integer whose bytes are in its order.
fn ordered(i: i64) -> u64 {
    i.cast_unsigned() ^ (1 << 63)
}

/// [`key`]'s inverse: none for bytes that aren't one.
pub fn of_key(element: u32, key: &[u8]) -> Option<Array> {
    let number = |bytes: &[u8]| -> Option<i64> {
        let word = u64::from_be_bytes(bytes.try_into().ok()?);
        Some((word ^ (1 << 63)).cast_signed())
    };
    let mut elements = Vec::new();
    let mut rest = key;
    loop {
        let (tag, after) = rest.split_first()?;
        rest = match *tag {
            END => {
                rest = after;
                break;
            }
            NULL => {
                elements.push(None);
                after
            }
            VALUE => {
                let (value, after) = match element {
                    type_oid::TEXT | type_oid::VARCHAR | type_oid::NAME => {
                        let end = after.iter().position(|b| *b == END)?;
                        let (text, after) = after.split_at_checked(end)?;
                        let text = std::str::from_utf8(text).ok()?;
                        (Value::Text(text.to_owned()), after.get(1..)?)
                    }
                    type_oid::CHAR => {
                        let (c, after) = after.split_first()?;
                        (Value::Char(*c), after)
                    }
                    _ => {
                        let (bytes, after) = after.split_at_checked(8)?;
                        let i = number(bytes)?;
                        let value = match element {
                            type_oid::BOOL => Value::Bool(i != 0),
                            type_oid::INT4 => Value::Int4(i32::try_from(i).ok()?),
                            type_oid::INT8 => Value::Int8(i),
                            type_oid::OID | type_oid::REGCLASS => {
                                Value::Oid(u32::try_from(i).ok()?)
                            }
                            type_oid::TIMESTAMP | type_oid::TIMESTAMPTZ => Value::Timestamp(i),
                            _ => return None,
                        };
                        (value, after)
                    }
                };
                elements.push(Some(value));
                after
            }
            _ => return None,
        };
    }
    let (count, rest) = rest.split_first()?;
    let count = usize::from(*count);
    let words: Vec<i32> = rest
        .chunks(8)
        .map(|bytes| i32::try_from(number(bytes)?).ok())
        .collect::<Option<_>>()?;
    if words.len() != count.checked_mul(2)? {
        return None;
    }
    let (lengths, lowers) = words.split_at_checked(count)?;
    let dimensions = lengths
        .iter()
        .zip(lowers)
        .map(|(length, lower)| Dimension {
            lower: *lower,
            length: *length,
        })
        .collect();
    Some(Array {
        element,
        dimensions,
        elements,
    })
}

/// `array_position_common` (src/backend/utils/adt/array_userfuncs.c): where an element first is, from `start`
/// on, by the equality of the elements' type; none where it isn't.
pub fn position(
    array: &Array,
    sought: Option<&Value>,
    start: Option<i32>,
) -> Result<Option<i32>, Error> {
    let first = match array.dimensions.as_slice() {
        [] => return Ok(None),
        [one] => one.lower,
        _ => {
            return Err(Error::new(
                sqlstate::FEATURE_NOT_SUPPORTED,
                "searching for elements in multidimensional arrays is not supported".to_owned(),
            ));
        }
    };
    let trimmed = |value: &Value| match value {
        Value::Text(text) if array.element == type_oid::BPCHAR => {
            Value::Text(text.trim_end_matches(' ').to_owned())
        }
        value => value.clone(),
    };
    let sought = sought.map(trimmed);
    let mut at = first;
    for element in &array.elements {
        if start.is_none_or(|start| at >= start) && element.as_ref().map(trimmed) == sought {
            return Ok(Some(at));
        }
        at = at.saturating_add(1);
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(element: &str) -> Result<Value, Error> {
        Ok(Value::Text(element.to_owned()))
    }

    fn shown(value: &Value) -> Result<String, Error> {
        match value {
            Value::Text(text) => Ok(text.clone()),
            _ => Err(Error::unsupported("another type")),
        }
    }

    /// T-PG.12.2: hostile bytes are no key, and every array's key reads back as it.
    #[test]
    fn t_pg_12_2_a_key_reads_back_and_other_bytes_are_none() {
        for literal in [
            "{}",
            "{a}",
            "{NULL,\"\",\"a b\",\"NULL\",\"{x}\"}",
            "[2:3][-1:0]={{a,b},{c,NULL}}",
            "{{{a}},{{b}}}",
        ] {
            let array = input(type_oid::TEXT, literal, text).unwrap();
            let key = key(&array).unwrap();
            assert_eq!(of_key(type_oid::TEXT, &key), Some(array.clone()), "{literal}");
            assert_eq!(output(&array, shown).unwrap(), literal);
            for cut in 0..key.len() {
                assert_eq!(of_key(type_oid::TEXT, &key[..cut]), None, "{literal} at {cut}");
            }
        }
        assert_eq!(of_key(type_oid::TEXT, &[3]), None);
        assert_eq!(of_key(type_oid::TEXT, &[1, 0xff, 0, 0, 0]), None);
    }

    /// T-PG.12.2: keys are in `array_cmp`'s order: by the elements, a NULL after every value, a shorter array first.
    #[test]
    fn t_pg_12_2_keys_are_in_the_order_of_their_arrays() {
        let arrays = [
            "{}", "{a}", "{a,b}", "{a,b,c}", "{a,c}", "{a,NULL}", "{aa}", "{b}", "{NULL}",
        ];
        let keys: Vec<Vec<u8>> = arrays
            .map(|literal| key(&input(type_oid::TEXT, literal, text).unwrap()).unwrap())
            .to_vec();
        assert!(keys.is_sorted(), "{keys:?}");
    }
}
