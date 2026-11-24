//! COPY FROM STDIN in text format (REL_19_0 src/backend/commands/copy.c, copyfrom.c and copyfromparse.c): what
//! the statement says, and the rows of the data. Pure: the data comes in pieces of any size.

use amphora_pg_parser::tree::{Node, NodeId, Tree};

use crate::func::{self, Value};
use crate::utility::{table_name, undefined_relation, user_table};
use crate::{Column, Error, Session, sqlstate};

/// How much of a value or a line a CONTEXT shows (`MAX_COPY_DATA_DISPLAY`).
const SHOWN: usize = 100;

/// A COPY of rows from the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyIn {
    pub relation: u32,
    pub relname: String,
    /// Every column of the table, in order.
    pub columns: Vec<Column>,
    /// The columns a line's fields are of, in `columns`.
    pub fields: Vec<usize>,
    pub freeze: bool,
}

/// What COPY FROM STDIN says: `None` for a statement that is no COPY.
pub fn copy_in(
    tree: &Tree,
    statement: NodeId,
    session: &Session<'_>,
) -> Result<Option<CopyIn>, Error> {
    let Node::RawStmt {
        stmt: Some(stmt), ..
    } = tree.get(statement)?
    else {
        return Ok(None);
    };
    let Node::CopyStmt {
        relation,
        query,
        attlist,
        is_from,
        is_program,
        filename,
        options,
        where_clause,
    } = tree.get(*stmt)?
    else {
        return Ok(None);
    };
    if !is_from || query.is_some() {
        return Err(Error::unsupported("COPY TO"));
    }
    if *is_program || filename.is_some() {
        return Err(Error::unsupported("COPY from a file or a program"));
    }
    if where_clause.is_some() {
        return Err(Error::unsupported("COPY FROM with a WHERE"));
    }
    let table = table_name(tree, *relation)?;
    let oid = user_table(session, None, table)?.ok_or_else(|| undefined_relation(table))?;
    let columns = session.relations.columns(oid);
    let fields = fields(tree, attlist, table, &columns)?;
    let freeze = options_freeze(tree, options)?;
    Ok(Some(CopyIn {
        relation: oid,
        relname: table.to_owned(),
        columns,
        fields,
        freeze,
    }))
}

/// `CopyGetAttnums` (copy.c): the columns a COPY names, or the table's.
fn fields(
    tree: &Tree,
    attlist: &[NodeId],
    table: &str,
    columns: &[Column],
) -> Result<Vec<usize>, Error> {
    if attlist.is_empty() {
        return Ok((0..columns.len()).collect());
    }
    let mut fields = Vec::with_capacity(attlist.len());
    for name in attlist {
        let Node::String(name) = tree.get(*name)? else {
            return Err(Error::unsupported("this column"));
        };
        let at = columns
            .iter()
            .position(|c| c.name == *name)
            .ok_or_else(|| {
                Error::new(
                    sqlstate::UNDEFINED_COLUMN,
                    format!("column \"{name}\" of relation \"{table}\" does not exist"),
                )
            })?;
        if fields.contains(&at) {
            return Err(Error::new(
                sqlstate::DUPLICATE_COLUMN,
                format!("column \"{name}\" specified more than once"),
            ));
        }
        fields.push(at);
    }
    Ok(fields)
}

/// `ProcessCopyOptions` (copy.c), as far as FORMAT text and FREEZE.
fn options_freeze(tree: &Tree, options: &[NodeId]) -> Result<bool, Error> {
    let mut freeze = None;
    let mut format = false;
    for option in options {
        let Node::DefElem {
            defnamespace: None,
            defname,
            arg,
            defaction: 0,
            location,
        } = tree.get(*option)?
        else {
            return Err(Error::unsupported("this COPY option"));
        };
        let twice = || {
            Error::new(
                sqlstate::SYNTAX_ERROR,
                "conflicting or redundant options".to_owned(),
            )
            .at(*location)
        };
        match defname.as_str() {
            "format" => {
                if format {
                    return Err(twice());
                }
                format = true;
                let name = match arg.map(|id| tree.get(id)).transpose()? {
                    Some(Node::String(name)) => name.as_str(),
                    _ => return Err(Error::unsupported("this COPY format")),
                };
                match name {
                    "text" => {}
                    "csv" | "binary" | "json" => {
                        return Err(Error::unsupported(&format!("COPY in {name} format")));
                    }
                    other => {
                        return Err(Error::new(
                            sqlstate::INVALID_PARAMETER_VALUE,
                            format!("COPY format \"{other}\" not recognized"),
                        )
                        .at(*location));
                    }
                }
            }
            "freeze" => {
                if freeze.is_some() {
                    return Err(twice());
                }
                freeze = Some(boolean(tree, *arg)?);
            }
            "delimiter" | "null" | "default" | "header" | "quote" | "escape" | "force_quote"
            | "force_not_null" | "force_null" | "convert_selectively" | "encoding" | "on_error"
            | "log_verbosity" | "reject_limit" => {
                return Err(Error::unsupported(&format!("COPY's {defname}")));
            }
            other => {
                return Err(Error::new(
                    sqlstate::SYNTAX_ERROR,
                    format!("option \"{other}\" not recognized"),
                )
                .at(*location));
            }
        }
    }
    Ok(freeze.unwrap_or(false))
}

/// `defGetBoolean` (src/backend/commands/define.c): true where there is none.
fn boolean(tree: &Tree, arg: Option<NodeId>) -> Result<bool, Error> {
    let not_boolean = || {
        Error::new(
            sqlstate::SYNTAX_ERROR,
            "freeze requires a Boolean value".to_owned(),
        )
    };
    let Some(arg) = arg else {
        return Ok(true);
    };
    match tree.get(arg)? {
        Node::Integer(0) => Ok(false),
        Node::Integer(1) => Ok(true),
        Node::String(word) => match word.to_ascii_lowercase().as_str() {
            "true" | "on" => Ok(true),
            "false" | "off" => Ok(false),
            _ => Err(not_boolean()),
        },
        _ => Err(not_boolean()),
    }
}

/// How a line ends (`EolType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Eol {
    Unknown,
    Nl,
    Cr,
    CrNl,
}

/// A COPY's refusal of its data: what is wrong, and what its CONTEXT says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub error: Error,
    pub context: String,
}

/// What a read gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    /// A line, without its end.
    Line(Vec<u8>),
    /// Nothing until there is more data.
    More,
    /// There are no more lines.
    End,
}

/// The data's lines (`CopyReadLineText`).
pub struct Lines {
    held: Vec<u8>,
    at: usize,
    eol: Eol,
    ended: bool,
    /// The line read last, from 1 (`cur_lineno`).
    number: u64,
}

impl Default for Lines {
    fn default() -> Self {
        Self {
            held: Vec::new(),
            at: 0,
            eol: Eol::Unknown,
            ended: false,
            number: 0,
        }
    }
}

impl Lines {
    pub fn push(&mut self, data: &[u8]) {
        if self.ended {
            return;
        }
        if self.at > 0 {
            self.held.drain(..self.at);
            self.at = 0;
        }
        self.held.extend_from_slice(data);
    }

    /// How much of the data is held.
    pub fn held(&self) -> usize {
        self.held.len()
    }

    /// The line being read, from 1.
    pub fn number(&self) -> u64 {
        self.number
    }

    /// The next line: whole, or the rest of the data when `eof`. After `\.` or the end there are none.
    pub fn next_line(&mut self, eof: bool) -> Result<Read, Error> {
        if self.ended {
            return Ok(Read::End);
        }
        let found = self.find(eof);
        match found {
            Ok(Found::Line { end, next }) => {
                self.number = self.number.saturating_add(1);
                let line = self.held.get(self.at..end).unwrap_or_default().to_vec();
                self.at = next;
                Ok(Read::Line(line))
            }
            Ok(Found::More) => Ok(Read::More),
            Ok(Found::End) => {
                self.ended = true;
                self.number = self.number.saturating_add(1);
                Ok(Read::End)
            }
            Err(message) => {
                self.number = self.number.saturating_add(1);
                Err(Error::new(sqlstate::BAD_COPY_FILE_FORMAT, message))
            }
        }
    }

    /// Where the line from `at` ends, and where the next begins.
    fn find(&mut self, eof: bool) -> Result<Found, String> {
        let start = self.at;
        let byte = |i: usize| self.held.get(i).copied();
        let mut i = start;
        loop {
            let Some(c) = byte(i) else {
                return Ok(if !eof {
                    Found::More
                } else if i == start {
                    Found::End
                } else {
                    Found::Line { end: i, next: i }
                });
            };
            match c {
                b'\r' => {
                    let next = byte(i.saturating_add(1));
                    if matches!(self.eol, Eol::Unknown | Eol::CrNl) && next.is_none() && !eof {
                        return Ok(Found::More);
                    }
                    return match (self.eol, next) {
                        (Eol::Unknown | Eol::CrNl, Some(b'\n')) => {
                            self.eol = Eol::CrNl;
                            Ok(Found::Line {
                                end: i,
                                next: i.saturating_add(2),
                            })
                        }
                        (Eol::CrNl, _) => Err("literal carriage return found in data".to_owned()),
                        (Eol::Nl, _) => Err("literal carriage return found in data".to_owned()),
                        (Eol::Unknown | Eol::Cr, _) => {
                            self.eol = Eol::Cr;
                            Ok(Found::Line {
                                end: i,
                                next: i.saturating_add(1),
                            })
                        }
                    };
                }
                b'\n' => {
                    if matches!(self.eol, Eol::Cr | Eol::CrNl) {
                        return Err("literal newline found in data".to_owned());
                    }
                    self.eol = Eol::Nl;
                    return Ok(Found::Line {
                        end: i,
                        next: i.saturating_add(1),
                    });
                }
                b'\\' => {
                    let Some(escaped) = byte(i.saturating_add(1)) else {
                        if eof {
                            // A backslash at the very end is data.
                            let end = i.saturating_add(1);
                            return Ok(Found::Line { end, next: end });
                        }
                        return Ok(Found::More);
                    };
                    if escaped != b'.' {
                        i = i.saturating_add(2);
                        continue;
                    }
                    return self.end_marker(start, i.saturating_add(2), eof);
                }
                _ => i = i.saturating_add(1),
            }
        }
    }

    /// `\.` (copyfromparse.c): the end of the data, where it stands alone on a line, `after` it.
    fn end_marker(&self, start: usize, after: usize, eof: bool) -> Result<Found, String> {
        let byte = |i: usize| self.held.get(i).copied();
        let mut i = after;
        let mismatch = || Err("end-of-copy marker does not match previous newline style".to_owned());
        let not_alone = || Err("end-of-copy marker is not alone on its line".to_owned());
        // The byte past the data's end is a NUL, as the buffer is padded with one.
        let next = |i: &mut usize| match byte(*i) {
            Some(c) => {
                *i = i.saturating_add(1);
                Some(c)
            }
            None if eof => {
                *i = i.saturating_add(1);
                Some(0)
            }
            None => None,
        };
        if self.eol == Eol::CrNl {
            match next(&mut i) {
                None => return Ok(Found::More),
                Some(b'\n') => return mismatch(),
                Some(b'\r') => {}
                Some(_) => return not_alone(),
            }
        }
        match next(&mut i) {
            None => return Ok(Found::More),
            Some(c @ (b'\r' | b'\n')) => {
                if matches!(
                    (self.eol, c),
                    (Eol::Nl, b'\r') | (Eol::CrNl, b'\r') | (Eol::Cr, b'\n')
                ) {
                    return mismatch();
                }
            }
            Some(_) => return not_alone(),
        }
        // Any data on this line before the marker.
        if after.saturating_sub(2) > start {
            return not_alone();
        }
        Ok(Found::End)
    }
}

enum Found {
    Line { end: usize, next: usize },
    More,
    End,
}

impl CopyIn {
    /// Nothing about a row: `COPY t, line n`.
    pub fn at_line(&self, number: u64) -> String {
        format!("COPY {}, line {number}", self.relname)
    }

    /// `CopyFromErrorCallback` for a line that was read whole.
    pub fn at_text(&self, number: u64, line: &[u8]) -> String {
        format!("COPY {}, line {number}: \"{}\"", self.relname, shown(line))
    }

    /// The row of a line: every column's value, in `columns` order, of the line's fields.
    pub fn row(
        &self,
        number: u64,
        line: &[u8],
        session: &Session<'_>,
    ) -> Result<Vec<Option<Value>>, Refused> {
        let whole = |error: Error| Refused {
            error,
            context: self.at_text(number, line),
        };
        if let Some(error) = invalid_encoding(line) {
            return Err(Refused {
                error,
                context: self.at_line(number),
            });
        }
        let mut row: Vec<Option<Value>> = vec![None; self.columns.len()];
        let fields = split(line);
        // pg_verify_mbstr of what an escape made, as the line is taken apart.
        for text in fields.iter().flatten() {
            if let Some(error) = invalid_encoding(text) {
                return Err(whole(error));
            }
        }
        if fields.len() > self.fields.len() {
            return Err(whole(bad_format("extra data after last expected column".to_owned())));
        }
        for (position, at) in self.fields.iter().enumerate() {
            let Some(column) = self.columns.get(*at) else {
                return Err(whole(Error::internal("a column of the copied table is missing")));
            };
            let Some(field) = fields.get(position) else {
                return Err(whole(bad_format(format!(
                    "missing data for column \"{}\"",
                    column.name
                ))));
            };
            let Some(text) = field else { continue };
            let of_column = |error: Error| Refused {
                error,
                context: format!(
                    "COPY {}, line {number}, column {}: \"{}\"",
                    self.relname,
                    column.name,
                    shown(text)
                ),
            };
            let Ok(word) = std::str::from_utf8(text) else {
                return Err(of_column(Error::internal("a field isn't UTF-8")));
            };
            let value = func::input(column.type_oid, word, session)
                .and_then(|value| func::fitted(column.type_oid, column.typmod, value))
                .map_err(of_column)?;
            if let Some(slot) = row.get_mut(*at) {
                *slot = Some(value);
            }
        }
        // ExecConstraints: the first column, in the table's order, that is NOT NULL and has no value.
        for (column, value) in self.columns.iter().zip(&row) {
            if column.not_null && value.is_none() {
                return Err(whole(Error::new(
                    sqlstate::NOT_NULL_VIOLATION,
                    format!(
                        "null value in column \"{}\" of relation \"{}\" violates not-null constraint",
                        column.name, self.relname
                    ),
                )));
            }
        }
        Ok(row)
    }
}

fn bad_format(message: String) -> Error {
    Error::new(sqlstate::BAD_COPY_FILE_FORMAT, message)
}

/// What a CONTEXT shows of text (`CopyLimitPrintoutLength`): its first 100 bytes, cut at a character, and `...`.
fn shown(text: &[u8]) -> String {
    if text.len() <= SHOWN {
        return String::from_utf8_lossy(text).into_owned();
    }
    let mut end = SHOWN;
    while end > 0 && text.get(end).is_some_and(|b| b & 0xc0 == 0x80) {
        end = end.saturating_sub(1);
    }
    let cut = String::from_utf8_lossy(text.get(..end).unwrap_or_default()).into_owned();
    format!("{cut}...")
}

/// `pg_verify_mbstr` in UTF-8 (src/backend/utils/mb/mbutils.c): the refusal of text that isn't, or holds a NUL.
fn invalid_encoding(text: &[u8]) -> Option<Error> {
    let at = match std::str::from_utf8(text) {
        Ok(valid) => valid.as_bytes().iter().position(|b| *b == 0)?,
        Err(error) => {
            let nul = text
                .get(..error.valid_up_to())
                .and_then(|valid| valid.iter().position(|b| *b == 0));
            nul.unwrap_or(error.valid_up_to())
        }
    };
    let rest = text.get(at..).unwrap_or_default();
    // pg_utf_mblen of the first byte, `report_invalid_encoding_int`'s limit of 8 bytes.
    let width = match rest.first().copied().unwrap_or(0) {
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => 1,
    };
    let bytes: Vec<String> = rest.iter().take(width).map(|b| format!("0x{b:02x}")).collect();
    Some(Error::new(
        sqlstate::CHARACTER_NOT_IN_REPERTOIRE,
        format!(
            "invalid byte sequence for encoding \"UTF8\": {}",
            bytes.join(" ")
        ),
    ))
}

/// A line's fields (`CopyReadAttributesText`): `None` for `\N`, the rest with their escapes undone.
fn split(line: &[u8]) -> Vec<Option<Vec<u8>>> {
    let mut fields = Vec::new();
    let mut rest = line;
    loop {
        let mut field = Vec::new();
        let mut at = 0usize;
        let mut ended = false;
        let start = rest;
        while let Some(&c) = rest.get(at) {
            at = at.saturating_add(1);
            if c == b'\t' {
                ended = true;
                break;
            }
            if c != b'\\' {
                field.push(c);
                continue;
            }
            let Some(&escaped) = rest.get(at) else {
                // The C code stops at a backslash at the line's end, and adds nothing.
                break;
            };
            at = at.saturating_add(1);
            let digit = |b: Option<&u8>, radix: u32| b.and_then(|b| char::from(*b).to_digit(radix));
            match escaped {
                b'0'..=b'7' => {
                    let mut value = u32::from(escaped).saturating_sub(u32::from(b'0'));
                    for _ in 0..2 {
                        let Some(d) = digit(rest.get(at), 8) else { break };
                        value = value.wrapping_mul(8).wrapping_add(d);
                        at = at.saturating_add(1);
                    }
                    field.push(u8::try_from(value & 0xff).unwrap_or(0));
                }
                b'x' if digit(rest.get(at), 16).is_some() => {
                    let mut value = 0u32;
                    for _ in 0..2 {
                        let Some(d) = digit(rest.get(at), 16) else { break };
                        value = value.wrapping_mul(16).wrapping_add(d);
                        at = at.saturating_add(1);
                    }
                    field.push(u8::try_from(value & 0xff).unwrap_or(0));
                }
                b'b' => field.push(0x08),
                b'f' => field.push(0x0c),
                b'n' => field.push(b'\n'),
                b'r' => field.push(b'\r'),
                b't' => field.push(b'\t'),
                b'v' => field.push(0x0b),
                other => field.push(other),
            }
        }
        let end = if ended { at.saturating_sub(1) } else { at };
        let raw = start.get(..end).unwrap_or_default();
        fields.push(if raw == b"\\N" { None } else { Some(field) });
        if !ended {
            return fields;
        }
        rest = start.get(at..).unwrap_or_default();
    }
}

