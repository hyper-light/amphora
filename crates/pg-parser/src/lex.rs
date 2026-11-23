//! scan.l's rule actions over flex's matching loop (crate::scan), and parser.c's `base_yylex` filter
//! above them: a statement's tokens exactly as PostgreSQL 19's grammar receives them (T-PG.4.x).
//! standard_conforming_strings is on (19 removed the alternative) and backslash_quote is its default
//! `safe_encoding` under UTF-8, so `\'` in an E'' string is an escaped quote.

use crate::scan::longest_match;
use crate::scan_tables::{INITIAL, XB, XC, XD, XDOLQ, XE, XEU, XH, XQ, XQS, XUI, XUS};
use crate::scan_tokens::{
    BCONST, BETWEEN, COLON_EQUALS, DOT_DOT, EQUALS_GREATER, FCONST, FIRST_P, FORMAT, FORMAT_LA,
    GREATER_EQUALS, ICONST, IDENT, ILIKE, IN_P, JSON, KEYWORDS, LAST_P, LESS_EQUALS, LIKE, NOT,
    NOT_EQUALS, NOT_LA, NULLS_LA, NULLS_P, OP, ORDINALITY, PARAM, SCONST, SIMILAR, TIME, TYPECAST,
    UESCAPE, UIDENT, USCONST, WITH, WITH_LA, WITHOUT, WITHOUT_LA, XCONST,
};

/// NAMEDATALEN: identifiers keep at most `NAMEDATALEN - 1` bytes.
const NAMEDATALEN: usize = 64;

/// What a token carries: an identifier's or literal's bytes, an integer, a keyword's name, or nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    None,
    Str(Vec<u8>),
    Int(i32),
    Keyword(&'static str),
}

/// One token as 19's grammar receives it: gram.h's number (a single character's is its byte, signed
/// as C's `char`), its byte offset in the statement, and its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub token: i32,
    pub offset: usize,
    pub value: Value,
}

/// 19's refusal: its SQLSTATE and the ErrorResponse cursor (1-based characters, 0 for none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanError {
    pub sqlstate: &'static str,
    pub cursor: usize,
    pub message: &'static str,
}

const SYNTAX_ERROR: &str = "42601";
const INVALID_ESCAPE_SEQUENCE: &str = "22025";
const CHARACTER_NOT_IN_REPERTOIRE: &str = "22021";
const INTERNAL_ERROR: &str = "XX000";

/// scanner_errposition: the 1-based character position of byte `at` (pg_mbstrlen_with_len + 1).
fn cursor(input: &[u8], at: usize) -> usize {
    input
        .iter()
        .take(at)
        .filter(|&&b| b & 0xC0 != 0x80)
        .count()
        .saturating_add(1)
}

fn fault() -> ScanError {
    ScanError {
        sqlstate: INTERNAL_ERROR,
        cursor: 0,
        message: "scanner tables fault",
    }
}

fn add(a: usize, b: usize) -> Result<usize, ScanError> {
    a.checked_add(b).ok_or_else(fault)
}

/// ScanKeywordLookup: the keyword whose name is `text` with ASCII letters lowered.
fn keyword(text: &[u8]) -> Option<(&'static str, i32)> {
    let lower: Vec<u8> = text.iter().map(u8::to_ascii_lowercase).collect();
    KEYWORDS
        .binary_search_by(|(k, _)| k.as_bytes().cmp(lower.as_slice()))
        .ok()
        .and_then(|i| KEYWORDS.get(i))
        .copied()
}

/// truncate_identifier: at most NAMEDATALEN - 1 bytes, cut at a character boundary (pg_mbcliplen).
fn truncate(mut ident: Vec<u8>) -> Vec<u8> {
    if ident.len() >= NAMEDATALEN {
        let mut keep = 0usize;
        let mut i = 0usize;
        while let Some(&b) = ident.get(i) {
            let l = utf8_len(b);
            let end = i.saturating_add(l);
            if end > NAMEDATALEN - 1 {
                break;
            }
            keep = end;
            i = end;
        }
        ident.truncate(keep);
    }
    ident
}

/// pg_utf_mblen.
fn utf8_len(b: u8) -> usize {
    match b {
        _ if b & 0x80 == 0 => 1,
        _ if b & 0xE0 == 0xC0 => 2,
        _ if b & 0xF0 == 0xE0 => 3,
        _ if b & 0xF8 == 0xF0 => 4,
        _ => 1,
    }
}

/// downcase_truncate_identifier under UTF-8: ASCII letters lowered, then truncated.
fn downcase_truncate(text: &[u8]) -> Vec<u8> {
    truncate(text.iter().map(u8::to_ascii_lowercase).collect())
}

/// pg_strtoint32_safe over a lexeme scan.l already shaped: digits with `_` between them, an optional
/// 0x/0o/0b prefix. `None` when it doesn't fit an int4.
fn strtoint32(text: &[u8]) -> Option<i32> {
    let (radix, digits) = match text {
        [b'0', b'x' | b'X', rest @ ..] => (16, rest),
        [b'0', b'o' | b'O', rest @ ..] => (8, rest),
        [b'0', b'b' | b'B', rest @ ..] => (2, rest),
        _ => (10, text),
    };
    let mut v: i32 = 0;
    let mut any = false;
    for &c in digits {
        if c == b'_' {
            continue;
        }
        let d = i32::try_from(char::from(c).to_digit(radix)?).ok()?;
        v = v.checked_mul(i32::try_from(radix).ok()?)?.checked_add(d)?;
        any = true;
    }
    if any { Some(v) } else { None }
}

/// process_integer_literal: ICONST when it fits an int4, else FCONST with its text.
fn integer(text: &[u8], offset: usize) -> Token {
    match strtoint32(text) {
        Some(v) => Token {
            token: ICONST,
            offset,
            value: Value::Int(v),
        },
        None => Token {
            token: FCONST,
            offset,
            value: Value::Str(text.to_vec()),
        },
    }
}

/// C's `char` as gram.y's single-character token: signed, as on the oracle's platform (x86-64).
fn char_token(b: u8) -> i32 {
    i32::from(i8::from_ne_bytes([b]))
}

/// unicode_to_utf8 for a code point is_valid_unicode_codepoint accepted.
fn push_utf8(out: &mut Vec<u8>, c: u32) {
    if let Some(ch) = char::from_u32(c) {
        let mut buf = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
    }
}

fn valid_codepoint(c: u32) -> bool {
    c > 0 && c <= 0x10FFFF
}

fn surrogate_first(c: u32) -> bool {
    (0xD800..=0xDBFF).contains(&c)
}

fn surrogate_second(c: u32) -> bool {
    (0xDC00..=0xDFFF).contains(&c)
}

fn surrogate_pair(first: u32, second: u32) -> u32 {
    (first & 0x3FF)
        .saturating_mul(0x400)
        .saturating_add(0x10000)
        .saturating_add(second & 0x3FF)
}

fn hex_value(digits: &[u8]) -> Option<u32> {
    digits.iter().try_fold(0u32, |v, &c| {
        v.checked_mul(16)?.checked_add(char::from(c).to_digit(16)?)
    })
}

/// scan.l's `core_yylex`: one token per call, `None` at the end of input.
struct Core<'a> {
    input: &'a [u8],
    pos: usize,
    condition: usize,
    lloc: usize,
    lit: Vec<u8>,
    xcdepth: u32,
    state_before_str_stop: usize,
    saw_non_ascii: bool,
    utf16_first_part: u32,
    dolqstart: Vec<u8>,
}

impl<'a> Core<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            pos: 0,
            condition: INITIAL,
            lloc: 0,
            lit: Vec::new(),
            xcdepth: 0,
            state_before_str_stop: INITIAL,
            saw_non_ascii: false,
            utf16_first_part: 0,
            dolqstart: Vec::new(),
        }
    }

    /// scanner_yyerror: a syntax error at yylloc.
    fn error(&self, message: &'static str) -> ScanError {
        ScanError {
            sqlstate: SYNTAX_ERROR,
            cursor: cursor(self.input, self.lloc),
            message,
        }
    }

    fn token(&self, token: i32, value: Value) -> Token {
        Token {
            token,
            offset: self.lloc,
            value,
        }
    }

    /// The literal a quote ended (scan.l's `<xqs>` return), once xqs knows no continuation follows.
    fn end_literal(&mut self) -> Result<Token, ScanError> {
        self.condition = INITIAL;
        let lit = std::mem::take(&mut self.lit);
        let token = match self.state_before_str_stop {
            XB => BCONST,
            XH => XCONST,
            XQ | XE => {
                if self.saw_non_ascii && (lit.contains(&0) || std::str::from_utf8(&lit).is_err()) {
                    return Err(ScanError {
                        sqlstate: CHARACTER_NOT_IN_REPERTOIRE,
                        cursor: 0,
                        message: "invalid byte sequence for encoding \"UTF8\"",
                    });
                }
                SCONST
            }
            XUS => USCONST,
            _ => return Err(self.error("unhandled previous state in xqs")),
        };
        Ok(self.token(token, Value::Str(lit)))
    }

    /// addunicode.
    fn add_unicode(&mut self, c: u32) -> Result<(), ScanError> {
        if !valid_codepoint(c) {
            return Err(self.error("invalid Unicode escape value"));
        }
        push_utf8(&mut self.lit, c);
        Ok(())
    }

    /// scan.l's `<<EOF>>` rules.
    fn end_of_input(&mut self) -> Result<Option<Token>, ScanError> {
        match self.condition {
            INITIAL => {
                self.lloc = self.input.len();
                Ok(None)
            }
            XC => Err(self.error("unterminated /* comment")),
            XB => Err(self.error("unterminated bit string literal")),
            XH => Err(self.error("unterminated hexadecimal string literal")),
            XQ | XE | XUS => Err(self.error("unterminated quoted string")),
            XQS => self.end_literal().map(Some),
            XEU => {
                self.lloc = self.input.len();
                Err(self.error("invalid Unicode surrogate pair"))
            }
            XDOLQ => Err(self.error("unterminated dollar-quoted string")),
            XD | XUI => Err(self.error("unterminated quoted identifier")),
            _ => Err(fault()),
        }
    }

    fn next(&mut self) -> Result<Option<Token>, ScanError> {
        loop {
            let input = self.input;
            let rest = input.get(self.pos..).ok_or_else(fault)?;
            if rest.is_empty() {
                return self.end_of_input();
            }
            let (rule, len) = longest_match(self.condition, rest).ok_or_else(fault)?;
            if len == 0 {
                return Err(fault());
            }
            let start = self.pos;
            let text = rest.get(..len).ok_or_else(fault)?;
            self.pos = add(start, len)?;
            if let Some(token) = self.action(rule, start, text)? {
                return Ok(Some(token));
            }
        }
    }

    /// yyless(n): keep the match's first `n` bytes.
    fn yyless(&mut self, start: usize, n: usize) -> Result<(), ScanError> {
        self.pos = add(start, n)?;
        Ok(())
    }

    /// One of scan.l's 72 rules, numbered as flex numbers them.
    fn action(&mut self, rule: u16, start: usize, text: &[u8]) -> Result<Option<Token>, ScanError> {
        let first = text.first().copied().unwrap_or(0);
        match rule {
            // {whitespace}; <xc>{xcinside}, {op_chars}, \*+
            1 | 5..=7 => {}
            // {xcstart}
            2 => {
                self.lloc = start;
                self.xcdepth = 0;
                self.condition = XC;
                self.yyless(start, 2)?;
            }
            // <xc>{xcstart}
            3 => {
                self.xcdepth = self.xcdepth.saturating_add(1);
                self.yyless(start, 2)?;
            }
            // <xc>{xcstop}
            4 => {
                if self.xcdepth == 0 {
                    self.condition = INITIAL;
                } else {
                    self.xcdepth = self.xcdepth.saturating_sub(1);
                }
            }
            // {xbstart}
            8 => {
                self.lloc = start;
                self.condition = XB;
                self.lit.clear();
                self.lit.push(b'b');
            }
            // <xh>{xhinside} | <xb>{xbinside}; <xq,xus>{xqinside}; <xe>{xeinside}; <xdolq>{dolqinside}, {dolqfailed}; <xd,xui>{xdinside}
            9 | 10 | 21 | 22 | 35 | 36 | 43 => self.lit.extend_from_slice(text),
            // {xhstart}
            11 => {
                self.lloc = start;
                self.condition = XH;
                self.lit.clear();
                self.lit.push(b'x');
            }
            // {xnstart}: only the n, as NCHAR when it's a keyword
            12 => {
                self.lloc = start;
                self.yyless(start, 1)?;
                return Ok(Some(match keyword(b"nchar") {
                    Some((name, token)) => self.token(token, Value::Keyword(name)),
                    None => self.token(IDENT, Value::Str(b"n".to_vec())),
                }));
            }
            // {xqstart}, {xestart}, {xusstart}
            13..=15 => {
                self.saw_non_ascii = false;
                self.lloc = start;
                self.condition = match rule {
                    13 => XQ,
                    14 => XE,
                    _ => XUS,
                };
                self.lit.clear();
            }
            // <xb,xh,xq,xe,xus>{quote}
            16 => {
                self.state_before_str_stop = self.condition;
                self.condition = XQS;
            }
            // <xqs>{quotecontinue}
            17 => self.condition = self.state_before_str_stop,
            // <xqs>{quotecontinuefail} | <xqs>{other}
            18 | 19 => {
                self.yyless(start, 0)?;
                return self.end_literal().map(Some);
            }
            // <xq,xe,xus>{xqdouble}
            20 => self.lit.push(b'\''),
            // <xe>{xeunicode}
            23 => {
                let c = hex_value(text.get(2..).ok_or_else(fault)?).ok_or_else(fault)?;
                let saved = self.lloc;
                self.lloc = start;
                if surrogate_first(c) {
                    self.utf16_first_part = c;
                    self.condition = XEU;
                } else if surrogate_second(c) {
                    return Err(self.error("invalid Unicode surrogate pair"));
                } else {
                    self.add_unicode(c)?;
                }
                self.lloc = saved;
            }
            // <xeu>{xeunicode}
            24 => {
                let c = hex_value(text.get(2..).ok_or_else(fault)?).ok_or_else(fault)?;
                let saved = self.lloc;
                self.lloc = start;
                if !surrogate_second(c) {
                    return Err(self.error("invalid Unicode surrogate pair"));
                }
                self.add_unicode(surrogate_pair(self.utf16_first_part, c))?;
                self.lloc = saved;
                self.condition = XE;
            }
            // <xeu>. | <xeu>\n
            25 | 26 => {
                self.lloc = start;
                return Err(self.error("invalid Unicode surrogate pair"));
            }
            // <xe,xeu>{xeunicodefail}
            27 => {
                self.lloc = start;
                return Err(ScanError {
                    sqlstate: INVALID_ESCAPE_SEQUENCE,
                    cursor: cursor(self.input, start),
                    message: "invalid Unicode escape",
                });
            }
            // <xe>{xeescape}: unescape_single_char
            28 => {
                let c = text.get(1).copied().ok_or_else(fault)?;
                let out = match c {
                    b'b' => 0x08,
                    b'f' => 0x0C,
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b't' => b'\t',
                    b'v' => 0x0B,
                    _ => {
                        if c == 0 || c >= 0x80 {
                            self.saw_non_ascii = true;
                        }
                        c
                    }
                };
                self.lit.push(out);
            }
            // <xe>{xeoctesc}, {xehexesc}: strtoul into an unsigned char
            29 | 30 => {
                let (digits, radix) = if rule == 29 {
                    (text.get(1..), 8)
                } else {
                    (text.get(2..), 16)
                };
                let v = digits
                    .ok_or_else(fault)?
                    .iter()
                    .try_fold(0u32, |v, &c| {
                        v.checked_mul(radix)?
                            .checked_add(char::from(c).to_digit(radix)?)
                    })
                    .ok_or_else(fault)?;
                let c = v.to_le_bytes().first().copied().unwrap_or(0);
                self.lit.push(c);
                if c == 0 || c >= 0x80 {
                    self.saw_non_ascii = true;
                }
            }
            // <xe>.; <xdolq>.
            31 | 37 => self.lit.push(first),
            // {dolqdelim}
            32 => {
                self.lloc = start;
                self.dolqstart = text.to_vec();
                self.condition = XDOLQ;
                self.lit.clear();
            }
            // {dolqfailed}
            33 => {
                self.lloc = start;
                self.yyless(start, 1)?;
                return Ok(Some(self.token(char_token(first), Value::None)));
            }
            // <xdolq>{dolqdelim}
            34 => {
                if text == self.dolqstart.as_slice() {
                    self.dolqstart.clear();
                    self.condition = INITIAL;
                    let lit = std::mem::take(&mut self.lit);
                    return Ok(Some(self.token(SCONST, Value::Str(lit))));
                }
                let keep = text.len().saturating_sub(1);
                self.lit
                    .extend_from_slice(text.get(..keep).ok_or_else(fault)?);
                self.yyless(start, keep)?;
            }
            // {xdstart}, {xuistart}
            38 | 39 => {
                self.lloc = start;
                self.condition = if rule == 38 { XD } else { XUI };
                self.lit.clear();
            }
            // <xd>{xdstop}, <xui>{dquote}
            40 | 41 => {
                self.condition = INITIAL;
                if self.lit.is_empty() {
                    return Err(self.error("zero-length delimited identifier"));
                }
                let lit = std::mem::take(&mut self.lit);
                return Ok(Some(if rule == 40 {
                    self.token(IDENT, Value::Str(truncate(lit)))
                } else {
                    self.token(UIDENT, Value::Str(lit))
                }));
            }
            // <xd,xui>{xddouble}
            42 => self.lit.push(b'"'),
            // {xufailed}
            44 => {
                self.lloc = start;
                self.yyless(start, 1)?;
                return Ok(Some(self.token(
                    IDENT,
                    Value::Str(downcase_truncate(text.get(..1).ok_or_else(fault)?)),
                )));
            }
            45..=52 => {
                self.lloc = start;
                let token = match rule {
                    45 => TYPECAST,
                    46 => DOT_DOT,
                    47 => COLON_EQUALS,
                    48 => EQUALS_GREATER,
                    49 => LESS_EQUALS,
                    50 => GREATER_EQUALS,
                    _ => NOT_EQUALS,
                };
                return Ok(Some(self.token(token, Value::None)));
            }
            // {self}, {other}
            53 | 72 => {
                self.lloc = start;
                return Ok(Some(self.token(char_token(first), Value::None)));
            }
            54 => return self.operator(start, text).map(Some),
            // {param}
            55 => {
                self.lloc = start;
                let Some(v) = strtoint32(text.get(1..).ok_or_else(fault)?) else {
                    return Err(self.error("parameter number too large"));
                };
                return Ok(Some(self.token(PARAM, Value::Int(v))));
            }
            56 => {
                self.lloc = start;
                return Err(self.error("trailing junk after parameter"));
            }
            57..=60 => {
                self.lloc = start;
                return Ok(Some(integer(text, start)));
            }
            61..=63 => {
                self.lloc = start;
                return Err(self.error(match rule {
                    61 => "invalid hexadecimal integer",
                    62 => "invalid octal integer",
                    _ => "invalid binary integer",
                }));
            }
            // {numeric}, {real}
            64 | 66 => {
                self.lloc = start;
                return Ok(Some(self.token(FCONST, Value::Str(text.to_vec()))));
            }
            // {numericfail}: the integer before `..`
            65 => {
                let keep = text.len().saturating_sub(2);
                self.yyless(start, keep)?;
                self.lloc = start;
                return Ok(Some(integer(text.get(..keep).ok_or_else(fault)?, start)));
            }
            67..=70 => {
                self.lloc = start;
                return Err(self.error("trailing junk after numeric literal"));
            }
            // {identifier}
            71 => {
                self.lloc = start;
                return Ok(Some(match keyword(text) {
                    Some((name, token)) => self.token(token, Value::Keyword(name)),
                    None => self.token(IDENT, Value::Str(downcase_truncate(text))),
                }));
            }
            _ => return Err(fault()),
        }
        Ok(None)
    }

    /// The {operator} rule: cut at a comment start, drop trailing + and - unless a character only
    /// an operator can hold comes first, and return the one- and two-character ones as theirs.
    fn operator(&mut self, start: usize, text: &[u8]) -> Result<Token, ScanError> {
        let find = |pat: &[u8]| text.windows(2).position(|w| w == pat);
        let mut nchars = match (find(b"/*"), find(b"--")) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => text.len(),
        };
        let ends_sign = |n: usize| {
            matches!(
                n.checked_sub(1).and_then(|i| text.get(i)),
                Some(b'+' | b'-')
            )
        };
        if nchars > 1 && ends_sign(nchars) {
            let special = text
                .iter()
                .take(nchars.saturating_sub(1))
                .any(|c| b"~!@#^&|`?%".contains(c));
            if !special {
                loop {
                    nchars = nchars.saturating_sub(1);
                    if !(nchars > 1 && ends_sign(nchars)) {
                        break;
                    }
                }
            }
        }
        self.lloc = start;
        let text = text.get(..nchars).ok_or_else(fault)?;
        if nchars < self.pos.saturating_sub(start) {
            self.yyless(start, nchars)?;
            if let [c] = text
                && b",()[].;:+-*/%^<>=".contains(c)
            {
                return Ok(self.token(char_token(*c), Value::None));
            }
            let two = match text {
                b"=>" => Some(EQUALS_GREATER),
                b">=" => Some(GREATER_EQUALS),
                b"<=" => Some(LESS_EQUALS),
                b"<>" | b"!=" => Some(NOT_EQUALS),
                _ => None,
            };
            if let Some(token) = two {
                return Ok(self.token(token, Value::None));
            }
        }
        if nchars >= NAMEDATALEN {
            return Err(self.error("operator too long"));
        }
        Ok(self.token(OP, Value::Str(text.to_vec())))
    }
}

/// check_uescapechar.
fn uescape_char_ok(c: u8) -> bool {
    !(c.is_ascii_hexdigit()
        || matches!(
            c,
            b'+' | b'\'' | b'"' | b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C
        ))
}

/// parser.c's str_udeescape: `\XXXX` and `\+XXXXXX` (with `escape` for `\`) into UTF-8, surrogate
/// pairs joined; errors point at `position + 3 + ` the escape's byte in the literal, as 19's do.
fn udeescape(input: &[u8], s: &[u8], escape: u8, position: usize) -> Result<Vec<u8>, ScanError> {
    let at = |i: usize| ScanError {
        sqlstate: SYNTAX_ERROR,
        cursor: cursor(input, i.saturating_add(position).saturating_add(3)),
        message: "",
    };
    let pair = |i: usize| ScanError {
        message: "invalid Unicode surrogate pair",
        ..at(i)
    };
    let mut out = Vec::with_capacity(s.len());
    let mut pair_first = 0u32;
    let mut i = 0usize;
    let hex = |from: usize, n: usize| -> Option<u32> {
        let d = s.get(from..from.checked_add(n)?)?;
        d.iter()
            .all(u8::is_ascii_hexdigit)
            .then(|| hex_value(d))
            .flatten()
    };
    while let Some(&c) = s.get(i) {
        if c != escape {
            if pair_first != 0 {
                return Err(pair(i));
            }
            out.push(c);
            i = add(i, 1)?;
            continue;
        }
        let next = s.get(i.saturating_add(1)).copied();
        if next == Some(escape) {
            if pair_first != 0 {
                return Err(pair(i));
            }
            out.push(escape);
            i = add(i, 2)?;
            continue;
        }
        let (unicode, step) = if let Some(u) = hex(i.saturating_add(1), 4) {
            (u, 5)
        } else if next == Some(b'+')
            && let Some(u) = hex(i.saturating_add(2), 6)
        {
            (u, 8)
        } else {
            return Err(ScanError {
                message: "invalid Unicode escape",
                ..at(i)
            });
        };
        if !valid_codepoint(unicode) {
            return Err(ScanError {
                message: "invalid Unicode escape value",
                ..at(i)
            });
        }
        let mut unicode = unicode;
        if pair_first != 0 {
            if !surrogate_second(unicode) {
                return Err(pair(i));
            }
            unicode = surrogate_pair(pair_first, unicode);
            pair_first = 0;
        } else if surrogate_second(unicode) {
            return Err(pair(i));
        }
        if surrogate_first(unicode) {
            pair_first = unicode;
        } else {
            push_utf8(&mut out, unicode);
        }
        i = add(i, step)?;
    }
    if pair_first != 0 {
        return Err(pair(i));
    }
    Ok(out)
}

/// parser.c's `base_yylex`: scan.l's tokens with the _LA lookaheads and U&'' / U&"" decoding.
pub struct Lexer<'a> {
    core: Core<'a>,
    /// `have_lookahead`: the token after the current one, `Some(None)` for the end of input.
    lookahead: Option<Option<Token>>,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a [u8]) -> Self {
        Self {
            core: Core::new(input),
            lookahead: None,
        }
    }

    /// The grammar's next token, `None` at the end.
    pub fn next_token(&mut self) -> Result<Option<Token>, ScanError> {
        let cur = match self.lookahead.take() {
            Some(t) => t,
            None => self.core.next()?,
        };
        let Some(mut cur) = cur else { return Ok(None) };
        if ![FORMAT, NOT, NULLS_P, WITH, WITHOUT, UIDENT, USCONST].contains(&cur.token) {
            return Ok(Some(cur));
        }
        let next = self.core.next()?;
        let next_token = next.as_ref().map_or(0, |t| t.token);
        let la = match cur.token {
            FORMAT if next_token == JSON => Some(FORMAT_LA),
            NOT if [BETWEEN, IN_P, LIKE, ILIKE, SIMILAR].contains(&next_token) => Some(NOT_LA),
            NULLS_P if [FIRST_P, LAST_P].contains(&next_token) => Some(NULLS_LA),
            WITH if [TIME, ORDINALITY].contains(&next_token) => Some(WITH_LA),
            WITHOUT if next_token == TIME => Some(WITHOUT_LA),
            _ => None,
        };
        if let Some(la) = la {
            cur.token = la;
        }
        if cur.token == UIDENT || cur.token == USCONST {
            let Value::Str(s) = std::mem::replace(&mut cur.value, Value::None) else {
                return Err(fault());
            };
            let decoded = if next_token == UESCAPE {
                let third = self.core.next()?;
                let escape = match third {
                    Some(Token {
                        token: SCONST,
                        value: Value::Str(e),
                        ..
                    }) => e,
                    _ => {
                        return Err(self
                            .core
                            .error("UESCAPE must be followed by a simple string literal"));
                    }
                };
                let ok = match escape.as_slice() {
                    [c] if uescape_char_ok(*c) => Some(*c),
                    _ => None,
                };
                let Some(esc) = ok else {
                    return Err(self.core.error("invalid Unicode escape character"));
                };
                udeescape(self.core.input, &s, esc, cur.offset)?
            } else {
                self.lookahead = Some(next);
                udeescape(self.core.input, &s, b'\\', cur.offset)?
            };
            if cur.token == UIDENT {
                cur.token = IDENT;
                cur.value = Value::Str(truncate(decoded));
            } else {
                cur.token = SCONST;
                cur.value = Value::Str(decoded);
            }
            return Ok(Some(cur));
        }
        self.lookahead = Some(next);
        Ok(Some(cur))
    }
}

/// A statement's tokens as the oracle's `--tokens` body prints them: `<token> <offset> <bytes> <value>\n`.
pub fn tokens(sql: &[u8]) -> Result<Vec<u8>, ScanError> {
    use std::io::Write;
    let mut lexer = Lexer::new(sql);
    let mut out = Vec::new();
    while let Some(t) = lexer.next_token()? {
        let int;
        let value: &[u8] = match &t.value {
            Value::None => b"",
            Value::Str(s) => s,
            Value::Keyword(k) => k.as_bytes(),
            Value::Int(v) => {
                int = v.to_string();
                int.as_bytes()
            }
        };
        let _ = write!(out, "{} {} {} ", t.token, t.offset, value.len());
        out.extend_from_slice(value);
        out.push(b'\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::tokens;

    fn lines(sql: &str) -> String {
        String::from_utf8(tokens(sql.as_bytes()).unwrap()).unwrap()
    }

    fn refused(sql: &str) -> (&'static str, usize) {
        let e = tokens(sql.as_bytes()).unwrap_err();
        (e.sqlstate, e.cursor)
    }

    /// T-PG.4.4: identifiers fold to lower case, keywords come back as their own tokens with their names.
    #[test]
    fn t_pg_4_4_identifiers_fold_and_keywords_resolve() {
        assert_eq!(lines("SELECT Ab"), "655 0 6 select\n258 7 2 ab\n");
        assert_eq!(lines("\"Ab\""), "258 0 2 Ab\n");
    }

    /// T-PG.4.4: an operator loses trailing + and - unless it holds a character only operators hold.
    #[test]
    fn t_pg_4_4_operator_trims_trailing_signs() {
        assert_eq!(lines("a=-1"), "258 0 1 a\n61 1 0 \n45 2 0 \n266 3 1 1\n");
        assert_eq!(lines("a@-1"), "258 0 1 a\n265 1 2 @-\n266 3 1 1\n");
        assert_eq!(lines("a<=/*c*/b"), "258 0 1 a\n272 1 0 \n258 8 1 b\n");
    }

    /// T-PG.4.4: E'' escapes, '' continuation and dollar quoting give the literal's bytes.
    #[test]
    fn t_pg_4_4_string_literals() {
        assert_eq!(lines("E'a\\n\\x41\\101'"), "261 0 4 a\nAA\n");
        assert_eq!(lines("'a'\n'b'"), "261 0 2 ab\n");
        assert_eq!(lines("$q$x$y$q$"), "261 0 3 x$y\n");
        assert_eq!(lines("U&'\\0041'"), "261 0 1 A\n");
        assert_eq!(lines("U&'!0041' UESCAPE '!'"), "261 0 1 A\n");
    }

    /// T-PG.4.4: the lookahead filter turns NOT before IN into NOT_LA.
    #[test]
    fn t_pg_4_4_not_in_is_not_la() {
        assert_eq!(
            lines("not in"),
            "775 0 3 not\n450 4 2 in\n"
                .replace("450", &crate::scan_tokens::IN_P.to_string())
                .replace("775", &crate::scan_tokens::NOT_LA.to_string())
        );
    }

    /// T-PG.4.4: unterminated literals and junk are 42601 at the lexeme's first character.
    #[test]
    fn t_pg_4_4_errors_point_at_the_lexeme() {
        assert_eq!(refused("select 'abc"), ("42601", 8));
        assert_eq!(refused("select /* x"), ("42601", 8));
        assert_eq!(refused("select 1abc"), ("42601", 8));
        assert_eq!(refused("select E'\\u12'"), ("22025", 10));
    }
}
