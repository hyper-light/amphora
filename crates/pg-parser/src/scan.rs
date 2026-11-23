//! flex's matching loop for `-CF` tables (its skeleton's `yy_match` and `yy_find_action`), over
//! scan.l's tables (crate::scan_tables): from a start condition, the longest match at the start of
//! the input and the rule it selects. scan.l's actions, its start-condition changes and `yyless`
//! are the scanner's, above this loop.

use crate::scan_tables::{NEXT, START, VERIFY};

/// The longest match of `input`'s head in start condition `condition`: the rule flex runs (scan.l's
/// rules numbered from 1 in order) and the bytes it takes. `None` for a condition scan.l doesn't
/// declare, or a state outside the tables (which would be a generator fault, not an input's).
/// Matching stops at the end of `input` as flex stops at its end-of-buffer NUL; an empty match is
/// the condition's end of input, scan.l's `<<EOF>>`.
pub fn longest_match(condition: usize, input: &[u8]) -> Option<(u16, usize)> {
    let start = condition.checked_mul(2)?.checked_add(1)?;
    let mut state = i64::from(*START.get(start)?);
    let mut taken = 0usize;
    while let Some(&c) = input.get(taken) {
        let at = usize::try_from(state.checked_add(i64::from(c))?).ok()?;
        if VERIFY.get(at).copied()? != u16::from(c) {
            break;
        }
        state = state.checked_add(i64::from(*NEXT.get(at)?))?;
        taken = taken.checked_add(1)?;
    }
    let action = NEXT.get(usize::try_from(state.checked_sub(1)?).ok()?).copied()?;
    Some((u16::try_from(action).ok()?, taken))
}

#[cfg(test)]
mod tests {
    use super::longest_match;
    use crate::scan_tables::{INITIAL, RULES, XQ};

    fn taken(condition: usize, input: &str) -> usize {
        let (rule, len) = longest_match(condition, input.as_bytes()).unwrap();
        assert!((1..=RULES).contains(&rule), "rule {rule} for {input:?}");
        len
    }

    /// T-PG.4.1: flex's longest match over 19's tables takes what scan.l's rules take.
    #[test]
    fn t_pg_4_1_longest_match_takes_scan_ls_lexemes() {
        assert_eq!(taken(INITIAL, "SELECT x"), 6);
        assert_eq!(taken(INITIAL, " \n\t x"), 4);
        assert_eq!(taken(INITIAL, "-- c\nx"), 5);
        assert_eq!(taken(INITIAL, "123abc "), 6, "integer_junk takes the junk with the number");
        assert_eq!(taken(INITIAL, "$1abc "), 5, "param_junk");
        assert_eq!(taken(INITIAL, ":: int"), 2);
        assert_eq!(taken(INITIAL, "'abc'"), 1, "xqstart, then BEGIN(xq)");
        assert_eq!(taken(XQ, "abc'"), 3, "xqinside");
        assert_eq!(taken(INITIAL, "<=> x"), 3, "an operator run");
    }

    /// T-PG.4.1: no input jams the tables: every byte from the start condition matches a rule.
    #[test]
    fn t_pg_4_1_every_byte_matches_a_rule_in_initial() {
        for b in 1..=255u8 {
            let (rule, len) = longest_match(INITIAL, &[b]).unwrap();
            assert!(len == 1 && (1..=RULES).contains(&rule), "byte {b}: rule {rule}, {len} bytes");
        }
    }
}
