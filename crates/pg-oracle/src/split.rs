//! A psql script split into the statements psql sends, as psql 19's lexer splits it
//! (`src/fe_utils/psqlscan.l`): a `;` ends a statement outside parentheses and outside a
//! `CREATE [OR REPLACE] {FUNCTION|PROCEDURE}`'s `BEGIN ... END`; quotes, dollar quotes and comments
//! hide what they hold; a backslash command takes the rest of its line, and `\g` and its kin send the
//! buffer; `COPY ... FROM STDIN` and `\copy ... from stdin` are followed by data up to a `\.` line,
//! which is no statement. psql variables (`:name`) are left as written: the corpus is the text.

/// psqlscan.l's `init_idents` (`char init_idents[8]`, psqlscan_int.h): the first identifiers
/// outside parentheses, by their letter.
const IDENTS: usize = 8;

#[derive(Default)]
struct Statement {
    text: Vec<u8>,
    idents: Vec<u8>,
    paren_depth: u32,
    begin_depth: u32,
}

impl Statement {
    fn push(&mut self, bytes: &[u8]) {
        self.text.extend_from_slice(bytes);
    }

    /// psqlscan_track_identifier and psqlscan_record_initial_keyword.
    fn identifier(&mut self, word: &[u8]) {
        if self.paren_depth != 0 {
            return;
        }
        if self.idents.len() < IDENTS {
            let lower = word.to_ascii_lowercase();
            let letter = match lower.as_slice() {
                b"create" | b"function" | b"procedure" | b"or" | b"replace" => {
                    lower.first().copied().unwrap_or(0)
                }
                b"copy" | b"from" | b"stdin" | b"stdout" => {
                    lower.first().map_or(0, u8::to_ascii_uppercase)
                }
                _ => 0,
            };
            self.idents.push(letter);
        }
        if self.is_create_routine() {
            if word.eq_ignore_ascii_case(b"begin") {
                self.begin_depth = self.begin_depth.saturating_add(1);
            } else if word.eq_ignore_ascii_case(b"case") {
                if self.begin_depth >= 1 {
                    self.begin_depth = self.begin_depth.saturating_add(1);
                }
            } else if word.eq_ignore_ascii_case(b"end") {
                self.begin_depth = self.begin_depth.saturating_sub(1);
            }
        }
    }

    fn ident(&self, at: usize) -> u8 {
        self.idents.get(at).copied().unwrap_or(0)
    }

    fn is_create_routine(&self) -> bool {
        let routine = |c: u8| c == b'f' || c == b'p';
        self.ident(0) == b'c'
            && (routine(self.ident(1))
                || (self.ident(1) == b'o' && self.ident(2) == b'r' && routine(self.ident(3))))
    }

    fn is_copy_from_stdin(&self) -> bool {
        self.ident(0) == b'C'
            && (1..IDENTS.saturating_sub(1))
                .find(|&at| self.ident(at) == b'F')
                .is_some_and(|at| self.ident(at.saturating_add(1)) == b'S')
    }

    fn outer(&self) -> bool {
        self.paren_depth == 0 && self.begin_depth == 0
    }
}

fn ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}

fn ident_cont(c: u8) -> bool {
    ident_start(c) || c.is_ascii_digit() || c == b'$'
}

/// `rest` cut after its first `n` bytes, or whole when shorter.
fn cut(rest: &[u8], n: usize) -> (&[u8], &[u8]) {
    rest.split_at_checked(n).unwrap_or((rest, &[]))
}

/// The line `rest` starts in, through its newline.
fn line(rest: &[u8]) -> (&[u8], &[u8]) {
    cut(
        rest,
        rest.iter()
            .position(|&c| c == b'\n')
            .map_or(rest.len(), |at| at.saturating_add(1)),
    )
}

/// A quoted run from its opening `quote`: `''` stands for one quote, and in an escape string a
/// backslash takes the next byte. Unterminated, it runs to the end, as psql then waits for more.
fn quoted(rest: &[u8], quote: u8, backslash: bool) -> (&[u8], &[u8]) {
    let mut at = 1usize;
    loop {
        match rest.get(at..) {
            Some([c, next, ..]) if *c == quote && *next == quote => at = at.saturating_add(2),
            Some([c, ..]) if *c == quote => return cut(rest, at.saturating_add(1)),
            Some([b'\\', _, ..]) if backslash => at = at.saturating_add(2),
            Some([_, ..]) => at = at.saturating_add(1),
            _ => return (rest, &[]),
        }
    }
}

/// A block comment, nested as PostgreSQL nests them.
fn block_comment(rest: &[u8]) -> (&[u8], &[u8]) {
    let mut depth = 0u32;
    let mut at = 0usize;
    loop {
        match rest.get(at..) {
            Some([b'/', b'*', ..]) => {
                depth = depth.saturating_add(1);
                at = at.saturating_add(2);
            }
            Some([b'*', b'/', ..]) => {
                depth = depth.saturating_sub(1);
                at = at.saturating_add(2);
                if depth == 0 {
                    return cut(rest, at);
                }
            }
            Some([_, ..]) => at = at.saturating_add(1),
            _ => return (rest, &[]),
        }
    }
}

/// A dollar quote's delimiter at the start of `rest` (`$$` or `$tag$`), if it is one.
fn dollar_delimiter(rest: &[u8]) -> Option<&[u8]> {
    let tail = rest.get(1..)?;
    let tag = tail
        .iter()
        .position(|&c| !ident_cont(c) || c == b'$')
        .unwrap_or(tail.len());
    let first = tail.first().copied()?;
    if tag > 0 && !ident_start(first) {
        return None;
    }
    (tail.get(tag) == Some(&b'$')).then(|| cut(rest, tag.saturating_add(2)).0)
}

fn dollar_quoted<'a>(rest: &'a [u8], delimiter: &[u8]) -> (&'a [u8], &'a [u8]) {
    let body = rest.get(delimiter.len()..).unwrap_or(&[]);
    match body.windows(delimiter.len()).position(|w| w == delimiter) {
        Some(at) => cut(rest, delimiter.len().saturating_mul(2).saturating_add(at)),
        None => (rest, &[]),
    }
}

/// COPY data after the line that sent the COPY: every line up to and including `\.`.
fn skip_copy_data(mut rest: &[u8]) -> &[u8] {
    while !rest.is_empty() {
        let (taken, after) = line(rest);
        rest = after;
        let text = taken.strip_suffix(b"\n").unwrap_or(taken);
        if text.strip_suffix(b"\r").unwrap_or(text) == b"\\." {
            break;
        }
    }
    rest
}

fn finish(out: &mut Vec<Vec<u8>>, statement: &mut Statement) {
    let mut text = std::mem::take(&mut statement.text);
    while text.last().is_some_and(u8::is_ascii_whitespace) {
        text.pop();
    }
    if !text.is_empty() {
        out.push(text);
    }
    *statement = Statement::default();
}

/// The statements psql sends for `script`, in order.
pub fn statements(script: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut statement = Statement::default();
    let mut rest = script;
    let mut copy_after_line = false;
    while let Some(&c) = rest.first() {
        let (taken, after) = match rest {
            [b'-', b'-', ..] => {
                let (comment, after) = line(rest);
                let comment = comment.strip_suffix(b"\n").unwrap_or(comment);
                (comment, rest.get(comment.len()..).unwrap_or(after))
            }
            [b'/', b'*', ..] => block_comment(rest),
            [b'\'', ..] => quoted(rest, b'\'', false),
            [b'"', ..] => quoted(rest, b'"', false),
            [b'$', ..] => match dollar_delimiter(rest) {
                Some(delimiter) => dollar_quoted(rest, delimiter),
                None => cut(rest, 1),
            },
            [b'\\', b';' | b':', ..] => {
                statement.push(rest.get(1..2).unwrap_or(&[]));
                if rest.get(1) == Some(&b';') && statement.outer() {
                    copy_after_line |= statement.is_copy_from_stdin();
                    statement.idents.clear();
                }
                rest = rest.get(2..).unwrap_or(&[]);
                continue;
            }
            [b'\\', ..] => {
                let (command, after) = line(rest);
                let name: Vec<u8> = command
                    .get(1..)
                    .unwrap_or(&[])
                    .iter()
                    .copied()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                let sends =
                    name.first() == Some(&b'g') || name == b"crosstabview" || name == b"watch";
                if sends {
                    copy_after_line |= statement.is_copy_from_stdin();
                    finish(&mut out, &mut statement);
                }
                let lower = command.to_ascii_lowercase();
                let words: Vec<&[u8]> = lower
                    .split(u8::is_ascii_whitespace)
                    .filter(|w| !w.is_empty())
                    .collect();
                let copy_stdin = name == b"copy"
                    && words
                        .windows(2)
                        .any(|pair| pair == [&b"from"[..], &b"stdin"[..]]);
                rest = after;
                if copy_stdin || std::mem::take(&mut copy_after_line) {
                    rest = skip_copy_data(rest);
                }
                continue;
            }
            [b'(', ..] => {
                statement.paren_depth = statement.paren_depth.saturating_add(1);
                cut(rest, 1)
            }
            [b')', ..] => {
                statement.paren_depth = statement.paren_depth.saturating_sub(1);
                cut(rest, 1)
            }
            [b';', ..] => {
                statement.push(b";");
                rest = rest.get(1..).unwrap_or(&[]);
                if statement.outer() {
                    copy_after_line |= statement.is_copy_from_stdin();
                    finish(&mut out, &mut statement);
                }
                continue;
            }
            [b'\n', ..] => {
                if !statement.text.is_empty() {
                    statement.push(b"\n");
                }
                rest = rest.get(1..).unwrap_or(&[]);
                if std::mem::take(&mut copy_after_line) {
                    rest = skip_copy_data(rest);
                }
                continue;
            }
            _ if ident_start(c) => {
                let len = rest
                    .iter()
                    .position(|&c| !ident_cont(c))
                    .unwrap_or(rest.len());
                let (word, after) = cut(rest, len);
                statement.identifier(word);
                statement.push(word);
                if word.eq_ignore_ascii_case(b"e") && after.first() == Some(&b'\'') {
                    let (string, after) = quoted(after, b'\'', true);
                    statement.push(string);
                    rest = after;
                    continue;
                }
                (&[][..], after)
            }
            _ => cut(rest, 1),
        };
        let whitespace_lead = statement.text.is_empty()
            && (taken.iter().all(u8::is_ascii_whitespace) || taken.starts_with(b"--"));
        if !whitespace_lead {
            statement.push(taken);
        }
        rest = after;
    }
    finish(&mut out, &mut statement);
    out
}

#[cfg(test)]
mod tests {
    use super::statements;

    fn split(script: &str) -> Vec<String> {
        statements(script.as_bytes())
            .into_iter()
            .map(|s| String::from_utf8(s).unwrap())
            .collect()
    }

    /// T-PG.3.2: psql's splitting, rule by rule.
    #[test]
    fn t_pg_3_2_semicolons_end_statements_outside_quotes_parens_and_routine_bodies() {
        assert_eq!(
            split("SELECT 1; SELECT ';' ;\nSELECT (1;2);"),
            ["SELECT 1;", "SELECT ';' ;", "SELECT (1;2);"]
        );
        assert_eq!(
            split("-- lead\nSELECT 1 -- tail ;\n;"),
            ["SELECT 1 -- tail ;\n;"]
        );
        assert_eq!(
            split("SELECT $$a;b$$, $q$;$q$, $1; SELECT E'\\';', \"a;\"\"\";"),
            [
                "SELECT $$a;b$$, $q$;$q$, $1;",
                "SELECT E'\\';', \"a;\"\"\";"
            ]
        );
        assert_eq!(
            split("SELECT /* a /* ; */ ; */ 1;"),
            ["SELECT /* a /* ; */ ; */ 1;"]
        );
        assert_eq!(
            split(
                "CREATE OR REPLACE FUNCTION f() RETURNS int BEGIN ATOMIC SELECT CASE WHEN true THEN 1 END; END;\nSELECT 2;"
            ),
            [
                "CREATE OR REPLACE FUNCTION f() RETURNS int BEGIN ATOMIC SELECT CASE WHEN true THEN 1 END; END;",
                "SELECT 2;"
            ]
        );
        assert_eq!(
            split("BEGIN; SELECT 1; END;"),
            ["BEGIN;", "SELECT 1;", "END;"]
        );
    }

    /// T-PG.3.2: backslash commands and COPY data are no statements.
    #[test]
    fn t_pg_3_2_backslash_commands_and_copy_data_are_not_sent() {
        assert_eq!(
            split("\\set x 1\nSELECT :x \\gset\n\\d t\nSELECT 2"),
            ["SELECT :x", "SELECT 2"]
        );
        assert_eq!(
            split("COPY t FROM stdin;\n1\t;\n\\.\nSELECT 1;"),
            ["COPY t FROM stdin;", "SELECT 1;"]
        );
        assert_eq!(
            split("COPY t (a) FROM STDIN WITH (format csv);\n1;\n\\.\nCOPY t TO stdout;"),
            [
                "COPY t (a) FROM STDIN WITH (format csv);",
                "COPY t TO stdout;"
            ]
        );
        assert_eq!(
            split("\\copy t from stdin\n1\n\\.\nSELECT 3;"),
            ["SELECT 3;"]
        );
        assert_eq!(split("SELECT 1 \\; SELECT 2;"), ["SELECT 1 ; SELECT 2;"]);
    }

    /// T-PG.3.2: hostile scripts end, unterminated quotes and comments run to the end.
    #[test]
    fn t_pg_3_2_unterminated_input_runs_to_the_end_and_never_panics() {
        assert_eq!(split("SELECT 'a;"), ["SELECT 'a;"]);
        assert_eq!(split("SELECT $x$;"), ["SELECT $x$;"]);
        assert_eq!(split("/* ; "), ["/* ;"]);
        for script in [
            "\\",
            "$",
            "e",
            "E'",
            "--",
            "(",
            ")))",
            "\\copy x from stdin",
            "COPY x FROM stdin;",
        ] {
            let _ = statements(script.as_bytes());
        }
    }
}
