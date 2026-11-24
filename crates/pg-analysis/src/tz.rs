//! A named time zone's changes of offset, from PostgreSQL 19's own time zone database
//! (vendor/postgres/src/timezone/data/tzdata.zi) as its zic compiles it (REL_19_0 src/timezone/zic.c),
//! and the `TimeZone` setting's other forms (`check_timezone`, src/backend/commands/variable.c).

use crate::datetime::{TimeZone, days};
use crate::{Error, sqlstate};

/// What PostgreSQL 19 ships: tzdata 2026f.
const TZDATA: &str = include_str!("../../../vendor/postgres/src/timezone/data/tzdata.zi");

/// How many years past the last one a zone names its changes are listed for, as outzone extends a zone
/// no POSIX rule covers (`YEARSPERREPEAT` and two more, zic.c): the last 400 of them repeat.
const YEARS_MORE: i64 = 402;

/// `ZIC_MAX` for a year: a rule with no end.
const FOREVER: i64 = i64::MAX;

const DAY: i64 = 86_400;

/// Which clock a time is on: `r_todisstd` and `r_todisut`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Clock {
    Wall,
    Standard,
    Universal,
}

/// A day of a month (`r_dycode`): a date, or the first weekday (0 is Sunday) on or after, or the last
/// on or before, a date; a month's last is on or before its last date.
#[derive(Clone, Copy)]
enum On {
    Date(i64),
    FirstFrom(i64, i64),
    LastBy(i64, i64),
    Last(i64),
}

#[derive(Clone, Copy)]
struct Moment {
    month: i64,
    on: On,
    at: i64,
    clock: Clock,
}

#[derive(Clone, Copy)]
struct Rule {
    from: i64,
    to: i64,
    when: Moment,
    save: i64,
}

/// What a zone line's RULES column says.
enum Saving<'a> {
    Fixed(i64),
    Rules(&'a str),
}

struct Era<'a> {
    standard: i64,
    saving: Saving<'a>,
    until: Option<(i64, Moment)>,
}

/// A value whose name a word starts (`byword`, zic.c): the first listed that has it as a prefix.
fn named(words: &[&str], word: &str) -> Option<i64> {
    let at = words.iter().position(|w| {
        w.get(..word.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(word))
    })?;
    i64::try_from(at).ok()
}

fn month(word: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    named(&MONTHS, word)?.checked_add(1)
}

fn weekday(word: &str) -> Option<i64> {
    const DAYS: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    named(&DAYS, word)
}

/// `gethms` (zic.c): a signed `h`, `h:m` or `h:m:s`, `-` for zero.
fn seconds(word: &str) -> Option<i64> {
    if word == "-" {
        return Some(0);
    }
    let (sign, digits) = match word.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, word),
    };
    let mut total = 0_i64;
    let mut unit = 3600_i64;
    for part in digits.split(':') {
        let n: i64 = part.parse().ok()?;
        total = total.checked_add(n.checked_mul(unit)?)?;
        unit = unit.checked_div(60)?;
    }
    total.checked_mul(sign)
}

/// A time of day and its clock: `w`, `s`, or `u`, `g` or `z` after it (rulesub, zic.c).
fn time_of_day(word: &str) -> Option<(i64, Clock)> {
    let (clock, digits) = match word.as_bytes().last() {
        Some(b's') => (Clock::Standard, word.strip_suffix('s')?),
        Some(b'w') => (Clock::Wall, word.strip_suffix('w')?),
        Some(b'u' | b'g' | b'z') => (Clock::Universal, word.get(..word.len().checked_sub(1)?)?),
        _ => (Clock::Wall, word),
    };
    Some((seconds(digits)?, clock))
}

fn on(word: &str) -> Option<On> {
    if let Some(day) = word.strip_prefix("last") {
        return Some(On::Last(weekday(day)?));
    }
    if let Some((day, date)) = word.split_once(">=") {
        return Some(On::FirstFrom(weekday(day)?, date.parse().ok()?));
    }
    if let Some((day, date)) = word.split_once("<=") {
        return Some(On::LastBy(weekday(day)?, date.parse().ok()?));
    }
    word.parse().ok().map(On::Date)
}

fn moment(words: &[&str]) -> Option<Moment> {
    let (at, clock) = match words.get(2) {
        Some(word) => time_of_day(word)?,
        None => (0, Clock::Wall),
    };
    Some(Moment {
        month: words.first().map_or(Some(1), |w| month(w))?,
        on: words.get(1).map_or(Some(On::Date(1)), |w| on(w))?,
        at,
        clock,
    })
}

/// `rpytime` (zic.c): the second, from 1970-01-01 on its own clock, a moment is in a year.
fn second(when: &Moment, year: i64) -> Option<i64> {
    // 1970-01-01 is 10957 days before 2000-01-01, and 2000-01-01 is a Saturday.
    let date = |month: i64, day: i64| days(year, month, day);
    let weekday_of = |d: i64| d.checked_add(6).map(|d| d.rem_euclid(7));
    let on_or_before = |d: i64, wanted: i64| {
        let back = weekday_of(d)?.checked_sub(wanted)?.rem_euclid(7);
        d.checked_sub(back)
    };
    let day = match when.on {
        On::Date(d) => date(when.month, d)?,
        On::FirstFrom(wanted, d) => {
            let first = date(when.month, d)?;
            let ahead = wanted.checked_sub(weekday_of(first)?)?.rem_euclid(7);
            first.checked_add(ahead)?
        }
        On::LastBy(wanted, d) => on_or_before(date(when.month, d)?, wanted)?,
        On::Last(wanted) => {
            let next = if when.month == 12 {
                days(year.checked_add(1)?, 1, 1)?
            } else {
                date(when.month.checked_add(1)?, 1)?
            };
            on_or_before(next.checked_sub(1)?, wanted)?
        }
    };
    day.checked_add(10_957)?
        .checked_mul(DAY)?
        .checked_add(when.at)
}

/// A second on a moment's clock as a second of UTC.
fn universal(at: i64, clock: Clock, standard: i64, save: i64) -> Option<i64> {
    match clock {
        Clock::Universal => Some(at),
        Clock::Standard => at.checked_sub(standard),
        Clock::Wall => at.checked_sub(standard)?.checked_sub(save),
    }
}

/// The rules of one name.
fn rules(name: &str) -> Option<Vec<Rule>> {
    let mut found = Vec::new();
    for line in TZDATA.lines() {
        let words: Vec<&str> = line.split_ascii_whitespace().collect();
        let ["R", rule, from, to, "-", rest @ ..] = words.as_slice() else {
            continue;
        };
        if *rule != name {
            continue;
        }
        let from: i64 = from.parse().ok()?;
        let to = match *to {
            "o" => from,
            "ma" => FOREVER,
            year => year.parse().ok()?,
        };
        let [month, on, at, save, _letters] = rest else {
            return None;
        };
        found.push(Rule {
            from,
            to,
            when: moment(&[month, on, at])?,
            save: seconds(save)?,
        });
    }
    Some(found)
}

/// A zone line's STDOFF, RULES, FORMAT and UNTIL.
fn era<'a>(words: &[&'a str]) -> Option<Era<'a>> {
    let [standard, saving, _format, until @ ..] = words else {
        return None;
    };
    let fixed = saving == &"-" || saving.contains(':') || saving.parse::<i64>().is_ok();
    Some(Era {
        standard: seconds(standard)?,
        saving: if fixed {
            Saving::Fixed(seconds(saving)?)
        } else {
            Saving::Rules(saving)
        },
        until: match until {
            [] => None,
            [year, rest @ ..] => Some((year.parse().ok()?, moment(rest)?)),
        },
    })
}

/// A zone's lines, under its name or a link's, in any case (`scan_directory_ci`, src/timezone/pgtz.c).
fn eras(name: &str) -> Option<Vec<Era<'static>>> {
    let mut wanted = name.to_owned();
    // A chain of links is no longer than the database.
    for _ in TZDATA.lines() {
        let mut lines = TZDATA.lines();
        let mut link = None;
        while let Some(line) = lines.next() {
            let words: Vec<&'static str> = line.split_ascii_whitespace().collect();
            match words.as_slice() {
                ["Z", zone, first @ ..] if zone.eq_ignore_ascii_case(&wanted) => {
                    let mut eras = vec![era(first)?];
                    while eras.last().is_some_and(|e| e.until.is_some()) {
                        let words: Vec<&'static str> =
                            lines.next()?.split_ascii_whitespace().collect();
                        eras.push(era(&words)?);
                    }
                    return Some(eras);
                }
                ["L", target, from] if from.eq_ignore_ascii_case(&wanted) => link = Some(*target),
                _ => {}
            }
        }
        wanted = link?.to_owned();
    }
    None
}

/// `outzone` (zic.c): a zone's offset before its first change, and its changes.
fn compile(eras: &[Era<'_>]) -> Option<TimeZone> {
    let first = eras.first()?;
    let mut changes: Vec<(i64, i64)> = Vec::new();
    let mut repeats = false;
    let mut start: Option<i64> = None;
    for e in eras {
        let standard = e.standard;
        let mut save = 0;
        let mut start_offset = standard;
        let mut use_start = start;
        match e.saving {
            Saving::Fixed(fixed) => {
                save = fixed;
                if let Some(at) = use_start.take() {
                    changes.push((at, standard.checked_add(save)?));
                }
            }
            Saving::Rules(name) => {
                let rules = rules(name)?;
                let mut last = match e.until {
                    Some((year, _)) => year,
                    None => rules
                        .iter()
                        .flat_map(|r| [r.from, r.to])
                        .filter(|year| *year != FOREVER)
                        .max()?,
                };
                if e.until.is_none() && rules.iter().any(|r| r.to == FOREVER) {
                    let named = eras.iter().filter_map(|e| e.until.as_ref());
                    let named = named.map(|(year, _)| *year).max().unwrap_or(last);
                    last = last.max(named).checked_add(YEARS_MORE)?;
                    repeats = true;
                }
                let year_one = rules.iter().map(|r| r.from).min()?;
                for year in year_one..=last {
                    let mut todo = Vec::new();
                    for rule in rules.iter().filter(|r| (r.from..=r.to).contains(&year)) {
                        todo.push((second(&rule.when, year)?, rule));
                    }
                    while !todo.is_empty() {
                        let mut earliest: Option<(i64, usize)> = None;
                        for (i, (at, rule)) in todo.iter().enumerate() {
                            let at = universal(*at, rule.when.clock, standard, save)?;
                            if earliest.is_none_or(|(e, _)| at < e) {
                                earliest = Some((at, i));
                            }
                        }
                        let (at, i) = earliest?;
                        let (_, rule) = todo.remove(i);
                        if let Some((until_year, when)) = &e.until {
                            let until = second(when, *until_year)?;
                            if at >= universal(until, when.clock, standard, save)? {
                                break;
                            }
                        }
                        save = rule.save;
                        let offset = standard.checked_add(save)?;
                        if use_start == Some(at) {
                            use_start = None;
                        }
                        if use_start.is_some_and(|start| at < start) {
                            start_offset = offset;
                            continue;
                        }
                        changes.push((at, offset));
                    }
                }
                if let Some(at) = use_start {
                    changes.push((at, start_offset));
                }
            }
        }
        if let Some((year, when)) = &e.until {
            start = Some(universal(second(when, *year)?, when.clock, standard, save)?);
        }
    }
    changes.sort_by_key(|c| c.0);
    // writezone (zic.c): a change no later on the clock than the one before it takes its place, and
    // one to the same offset is none.
    let before_first = first.standard.checked_add(match first.saving {
        Saving::Fixed(save) => save,
        Saving::Rules(_) => 0,
    })?;
    let mut kept: Vec<(i64, i64)> = Vec::with_capacity(changes.len());
    for (at, offset) in changes {
        let earlier = kept.len().checked_sub(2).and_then(|i| kept.get(i));
        let before_last = earlier.map_or(before_first, |c| c.1);
        if let Some(last) = kept.last_mut() {
            if at.checked_add(last.1)? <= last.0.checked_add(before_last)? {
                last.1 = offset;
                continue;
            }
            if last.1 == offset {
                continue;
            }
        } else if offset == before_first {
            continue;
        }
        kept.push((at, offset));
    }
    let mut narrow = Vec::with_capacity(kept.len());
    for (at, offset) in kept {
        narrow.push((at, i32::try_from(offset).ok()?));
    }
    Some(TimeZone::of(
        i32::try_from(before_first).ok()?,
        narrow,
        repeats,
    ))
}

/// The `TimeZone` a setting names (`check_timezone`, variable.c): a number of hours east of UTC, or a
/// zone of the database. INTERVAL and POSIX forms refuse `0A000`; no such zone is `22023`.
pub fn time_zone(setting: &str) -> Result<TimeZone, Error> {
    let invalid = || {
        Error::new(
            sqlstate::INVALID_PARAMETER_VALUE,
            format!("invalid value for parameter \"TimeZone\": \"{setting}\""),
        )
    };
    if let Some((sign, hours, tenths)) = hours(setting) {
        let east = hours
            .checked_mul(3600)
            .and_then(|h| h.checked_add(tenths))
            .and_then(|s| s.checked_mul(sign))
            .and_then(|s| i32::try_from(s).ok());
        // pg_tzset_offset (src/timezone/pgtz.c) makes a POSIX name of it, whose hours end at 167.
        return east
            .filter(|s| s.unsigned_abs() < 168 * 3600)
            .map(TimeZone::fixed)
            .ok_or_else(invalid);
    }
    if let Some(eras) = eras(setting) {
        return compile(&eras).ok_or_else(|| Error::internal("a time zone's rules can't be read"));
    }
    if setting.contains(|c: char| c.is_ascii_digit() || matches!(c, '<' | '\'')) {
        return Err(Error::unsupported(&format!("the time zone \"{setting}\"")));
    }
    Err(invalid())
}

/// A number of hours, with decimals: its sign, whole hours, and the seconds its decimals are.
fn hours(setting: &str) -> Option<(i64, i64, i64)> {
    let (sign, digits) = match setting.as_bytes() {
        [b'-', ..] => (-1, setting.get(1..)?),
        [b'+', ..] => (1, setting.get(1..)?),
        _ => (1, setting),
    };
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, "0"));
    let digits_only = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits_only(whole) || !digits_only(fraction) {
        return None;
    }
    let scale = 10_i64.checked_pow(u32::try_from(fraction.len()).ok()?)?;
    let part = fraction
        .parse::<i64>()
        .ok()?
        .checked_mul(3600)?
        .checked_div(scale)?;
    Some((sign, whole.parse().ok()?, part))
}
