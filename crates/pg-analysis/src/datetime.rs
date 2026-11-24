//! timestamp and timestamptz: their text (ISO input and output), their casts under the session's
//! `TimeZone`, and their precision (REL_19_0 src/backend/utils/adt/timestamp.c and datetime.c). Text
//! in a form other than ISO's refuses `0A000`: whether 19 reads it isn't known here.

use std::cmp::Ordering;

use crate::{Error, sqlstate};

const USECS: i64 = 1_000_000;
const DAY_SECS: i64 = 86_400;
const DAY_USECS: i64 = 86_400_000_000;
/// Seconds from 1970-01-01 to 2000-01-01, `POSTGRES_EPOCH_JDATE - UNIX_EPOCH_JDATE` days
/// (src/include/datatype/timestamp.h:234-235).
const EPOCH_SECS: i64 = 946_684_800;
/// `MIN_TIMESTAMP` and `END_TIMESTAMP` (timestamp.h:254-256).
const MIN_TIMESTAMP: i64 = -211_813_488_000_000_000;
const END_TIMESTAMP: i64 = 9_223_371_331_200_000_000;
/// `MAX_TZDISP_HOUR` (src/include/datatype/timestamp.h:144).
const MAX_ZONE_HOUR: i64 = 15;

/// The seconds in 400 years of the Gregorian calendar, after which every time zone rule repeats
/// (`SECSPERREPEAT`, src/timezone/tzfile.h).
const REPEAT_SECS: i64 = 12_622_780_800;

/// A `TimeZone` setting: the offsets from UTC, in seconds east, a zone has had and will have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeZone {
    /// The offset before the first change.
    first: i32,
    /// Each change: the second (from 1970-01-01 UTC) it takes effect, and the offset from then.
    changes: Vec<(i64, i32)>,
    /// Whether the last 400 years of changes repeat after them.
    repeats: bool,
}

impl TimeZone {
    /// A zone always this many seconds east of UTC (`pg_tzset_offset`, src/timezone/pgtz.c).
    pub const fn fixed(east: i32) -> Self {
        Self {
            first: east,
            changes: Vec::new(),
            repeats: false,
        }
    }

    pub(crate) fn of(first: i32, changes: Vec<(i64, i32)>, repeats: bool) -> Self {
        Self {
            first,
            changes,
            repeats,
        }
    }

    /// The second to look up for `at`: past the last change, the same second of an earlier 400 years.
    fn folded(&self, at: i64) -> (i64, i64) {
        let last = self.changes.last().map_or(i64::MAX, |c| c.0);
        if !self.repeats || at <= last {
            return (at, 0);
        }
        let cycles = at
            .checked_sub(last)
            .map_or(0, |past| past.div_euclid(REPEAT_SECS).saturating_add(1));
        let shift = cycles.saturating_mul(REPEAT_SECS);
        (at.saturating_sub(shift), shift)
    }

    /// The offset at a second of UTC (`pg_localtime`).
    fn at_utc(&self, at: i64) -> i32 {
        let (at, _) = self.folded(at);
        let after = self.changes.partition_point(|c| c.0 <= at);
        self.offset_before(after)
    }

    /// The offset in effect before the `n`th change.
    fn offset_before(&self, n: usize) -> i32 {
        n.checked_sub(1)
            .and_then(|i| self.changes.get(i))
            .map_or(self.first, |c| c.1)
    }

    /// The offset at a local second (`DetermineTimeZoneOffsetInternal`, datetime.c:1641): a time that
    /// a change skips is read by the offset before it, one that a change repeats by the offset after.
    fn at_local(&self, local: i64) -> Option<i32> {
        let (day_before, shift) = self.folded(local.checked_sub(DAY_SECS)?);
        let local = local.checked_sub(shift)?;
        let next = self.changes.partition_point(|c| c.0 <= day_before);
        let before = self.offset_before(next);
        let Some(&(boundary, after)) = self.changes.get(next) else {
            return Some(before);
        };
        let before_time = local.checked_sub(i64::from(before))?;
        let after_time = local.checked_sub(i64::from(after))?;
        Some(if before_time < boundary && after_time < boundary {
            before
        } else if before_time > boundary && after_time >= boundary {
            after
        } else if before_time > after_time {
            before
        } else {
            after
        })
    }
}

fn out_of_range() -> Error {
    Error::new(
        sqlstate::DATETIME_FIELD_OVERFLOW,
        "timestamp out of range".to_owned(),
    )
}

fn finite(t: i64) -> bool {
    t != i64::MIN && t != i64::MAX
}

/// `IS_VALID_TIMESTAMP` (timestamp.h:268).
fn valid(t: i64) -> Option<i64> {
    if (MIN_TIMESTAMP..END_TIMESTAMP).contains(&t) {
        Some(t)
    } else {
        None
    }
}

/// The local or UTC second a timestamp is in, from 1970-01-01.
fn unix_second(t: i64) -> Option<i64> {
    t.div_euclid(USECS).checked_add(EPOCH_SECS)
}

fn shifted(t: i64, secs: i64) -> Option<i64> {
    t.checked_add(secs.checked_mul(USECS)?).and_then(valid)
}

fn local_to_utc(t: i64, zone: &TimeZone) -> Option<i64> {
    let east = zone.at_local(unix_second(t)?)?;
    shifted(t, i64::from(east).checked_neg()?)
}

/// `timestamp2timestamptz` (timestamp.c): a timestamp read in the session's time zone.
pub fn to_timestamptz(t: i64, zone: &TimeZone) -> Result<i64, Error> {
    if !finite(t) {
        return Ok(t);
    }
    local_to_utc(t, zone).ok_or_else(out_of_range)
}

/// `timestamptz2timestamp` (timestamp.c): the session's time zone's clock at a timestamptz.
pub fn to_timestamp(t: i64, zone: &TimeZone) -> Result<i64, Error> {
    if !finite(t) {
        return Ok(t);
    }
    unix_second(t)
        .and_then(|s| shifted(t, i64::from(zone.at_utc(s))))
        .ok_or_else(out_of_range)
}

/// `timestamp_cmp_timestamptz_internal` (timestamp.c:2346): a timestamp that overflows as a timestamptz
/// is past every finite one, and short of the infinity there.
pub fn compare(t: i64, tz: i64, zone: &TimeZone) -> Ordering {
    if !finite(t) {
        return t.cmp(&tz);
    }
    match local_to_utc(t, zone) {
        Some(utc) => utc.cmp(&tz),
        None if t < 0 && tz != i64::MIN => Ordering::Less,
        None if t >= 0 && tz == i64::MAX => Ordering::Less,
        None => Ordering::Greater,
    }
}

/// `AdjustTimestampForTypmod` (timestamp.c): a timestamp rounded to a precision, half away from
/// 2000-01-01.
pub fn scale(t: i64, precision: i32) -> Result<i64, Error> {
    let Ok(precision) = u32::try_from(precision) else {
        return Ok(t);
    };
    let Some(digits) = 6_u32.checked_sub(precision) else {
        return Err(Error::new(
            sqlstate::INVALID_PARAMETER_VALUE,
            format!("timestamp({precision}) precision must be between 0 and 6"),
        ));
    };
    if !finite(t) || digits == 0 {
        return Ok(t);
    }
    let rounded = || {
        let unit = 10_i64.checked_pow(digits)?;
        let half = unit.checked_div(2)?;
        let magnitude = t.checked_abs()?.checked_add(half)?;
        let kept = magnitude.checked_sub(magnitude.checked_rem(unit)?)?;
        kept.checked_mul(t.signum())
    };
    rounded().ok_or_else(out_of_range)
}

/// `date2j` less 2000-01-01's: days from 2000-01-01 of a proleptic Gregorian date.
pub(crate) fn days(year: i64, month: i64, day: i64) -> Option<i64> {
    // A year from March, so the leap day is its last.
    let (y, m) = if month > 2 {
        (year, month.checked_sub(3)?)
    } else {
        (year.checked_sub(1)?, month.checked_add(9)?)
    };
    let in_year = m
        .checked_mul(153)?
        .checked_add(2)?
        .div_euclid(5)
        .checked_add(day.checked_sub(1)?)?;
    y.checked_mul(365)?
        .checked_add(y.div_euclid(4))?
        .checked_sub(y.div_euclid(100))?
        .checked_add(y.div_euclid(400))?
        .checked_add(in_year)?
        // Days from 0000-03-01 to 2000-01-01.
        .checked_sub(730_425)
}

/// `j2date` from days since 2000-01-01: the year, month and day.
pub(crate) fn date(days: i64) -> Option<(i64, i64, i64)> {
    let d = days.checked_add(730_425)?;
    let era = d.div_euclid(146_097);
    let doe = d.rem_euclid(146_097);
    let yoe = doe
        .checked_sub(doe.div_euclid(1460))?
        .checked_add(doe.div_euclid(36_524))?
        .checked_sub(doe.div_euclid(146_096))?
        .div_euclid(365);
    let in_year = doe.checked_sub(
        yoe.checked_mul(365)?
            .checked_add(yoe.div_euclid(4))?
            .checked_sub(yoe.div_euclid(100))?,
    )?;
    let mp = in_year.checked_mul(5)?.checked_add(2)?.div_euclid(153);
    let first = mp.checked_mul(153)?.checked_add(2)?.div_euclid(5);
    let day = in_year.checked_sub(first)?.checked_add(1)?;
    let month = if mp < 10 {
        mp.checked_add(3)?
    } else {
        mp.checked_sub(9)?
    };
    let year = era
        .checked_mul(400)?
        .checked_add(yoe)?
        .checked_add(i64::from(month <= 2))?;
    Some((year, month, day))
}

/// The date and time of day a literal names, and its zone's seconds east, if it has one.
struct Parsed {
    local: i64,
    zone: Option<i64>,
}

/// Why a literal names none.
enum Unread {
    /// Not ISO's form: `0A000`.
    Form,
    /// `DTERR_FIELD_OVERFLOW` and `DTERR_MD_FIELD_OVERFLOW` (datetime.c:4262): 22008.
    Field,
    /// `DTERR_TZDISP_OVERFLOW`: 22009.
    Zone,
}

/// Digits off the front of `s`, at least `min` and at most `max` of them, and the rest.
fn number(s: &str, min: usize, max: usize) -> Option<(i64, &str)> {
    let n = s.bytes().take(max).take_while(u8::is_ascii_digit).count();
    let (digits, rest) = s.split_at_checked(n).filter(|_| n >= min)?;
    Some((digits.parse().ok()?, rest))
}

/// A zone off the end of a literal: nothing, `Z`, or `+hh`, `+hh:mm` or `+hh:mm:ss`
/// (`DecodeTimezone`, datetime.c:3055).
fn zone(s: &str) -> Result<Option<i64>, Unread> {
    if s.is_empty() {
        return Ok(None);
    }
    if s.eq_ignore_ascii_case("z") {
        return Ok(Some(0));
    }
    let (sign, rest) = match s.as_bytes() {
        [b'+', ..] => (1_i64, s.get(1..)),
        [b'-', ..] => (-1, s.get(1..)),
        _ => return Err(Unread::Form),
    };
    let (hours, rest) = rest.and_then(|r| number(r, 1, 2)).ok_or(Unread::Form)?;
    let mut parts = [0_i64; 2];
    let mut rest = rest;
    for part in &mut parts {
        if let Some(more) = rest.strip_prefix(':') {
            (*part, rest) = number(more, 2, 2).ok_or(Unread::Form)?;
        }
    }
    if !rest.is_empty() {
        return Err(Unread::Form);
    }
    let [minutes, seconds] = parts;
    if hours > MAX_ZONE_HOUR || minutes > 59 || seconds > 59 {
        return Err(Unread::Zone);
    }
    let secs = || {
        hours
            .checked_mul(3600)?
            .checked_add(minutes.checked_mul(60)?)?
            .checked_add(seconds)?
            .checked_mul(sign)
    };
    secs().map(Some).ok_or(Unread::Zone)
}

/// `YYYY-MM-DD`, then a time after a space or `T`, then a zone: ISO 8601's form as DecodeDateTime
/// (datetime.c:771) reads it.
fn parse(s: &str) -> Result<Parsed, Unread> {
    let form = Unread::Form;
    let s = s.trim_matches(|c| matches!(c, ' ' | '\t'..='\r'));
    let (year, rest) = number(s, 4, 6).ok_or(form)?;
    let two = |rest: &str, sep: char| -> Result<(i64, String), Unread> {
        let rest = rest.strip_prefix(sep).ok_or(Unread::Form)?;
        let (n, rest) = number(rest, 1, 2).ok_or(Unread::Form)?;
        Ok((n, rest.to_owned()))
    };
    let (month, rest) = two(rest, '-')?;
    let (day, rest) = two(&rest, '-')?;
    let (mut hour, mut minute, mut second, mut micros) = (0, 0, 0, 0);
    let mut zone_text = rest.trim_start().to_owned();
    let time = match rest.as_bytes() {
        [b'T' | b't', ..] => rest.get(1..),
        [b' ', ..] => Some(rest.trim_start_matches(' ')),
        _ => None,
    };
    if let Some(time) = time.filter(|t| t.starts_with(|c: char| c.is_ascii_digit())) {
        let (h, rest) = number(time, 1, 2).ok_or(Unread::Form)?;
        let (m, mut rest) = two(rest, ':')?;
        (hour, minute) = (h, m);
        if rest.starts_with(':') {
            (second, rest) = two(&rest, ':')?;
            if let Some(fraction) = rest.strip_prefix('.') {
                let n = fraction.bytes().take_while(u8::is_ascii_digit).count();
                let (digits, after) = fraction.split_at_checked(n).ok_or(Unread::Form)?;
                if n > 6 {
                    return Err(Unread::Form);
                }
                micros = format!("{digits:0<6}").parse().map_err(|_| Unread::Form)?;
                rest = after.to_owned();
            }
        }
        zone_text = rest.trim_start_matches(' ').to_owned();
    }
    let zone = zone(&zone_text)?;
    // ValidateDate (datetime.c:2548) and DecodeTimeCommon (:2644), and the 60th second and 24:00:00
    // they allow.
    if year == 0 {
        return Err(Unread::Field);
    }
    let length = match month {
        4 | 6 | 9 | 11 => 30,
        2 if year.rem_euclid(4) == 0
            && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0) =>
        {
            29
        }
        2 => 28,
        1..=12 => 31,
        _ => return Err(Unread::Field),
    };
    let time = || {
        hour.checked_mul(3600)?
            .checked_add(i64::checked_mul(minute, 60)?)?
            .checked_add(second)?
            .checked_mul(USECS)?
            .checked_add(micros)
    };
    let time = time().ok_or(Unread::Field)?;
    if !(1..=length).contains(&day) || minute > 59 || second > 60 || time > DAY_USECS {
        return Err(Unread::Field);
    }
    let local = days(year, month, day)
        .and_then(|d| d.checked_mul(DAY_USECS))
        .and_then(|d| d.checked_add(time))
        .ok_or(Unread::Field)?;
    Ok(Parsed { local, zone })
}

/// The literals that aren't dates: DecodeSpecial's `EARLY`, `LATE` and `EPOCH` (datetime.c's datetktbl).
fn special(s: &str) -> Option<i64> {
    let s = s.trim_matches(|c| matches!(c, ' ' | '\t'..='\r'));
    match s.to_ascii_lowercase().as_str() {
        "infinity" | "+infinity" => Some(i64::MAX),
        "-infinity" => Some(i64::MIN),
        "epoch" => EPOCH_SECS.checked_mul(USECS)?.checked_neg(),
        _ => None,
    }
}

/// `DateTimeParseError` (datetime.c:4252).
fn unread(e: Unread, s: &str, name: &str) -> Error {
    match e {
        Unread::Form => Error::unsupported(&format!("the {name} literal \"{s}\"")),
        Unread::Field => Error::new(
            sqlstate::DATETIME_FIELD_OVERFLOW,
            format!("date/time field value out of range: \"{s}\""),
        ),
        Unread::Zone => Error::new(
            sqlstate::INVALID_TIME_ZONE_DISPLACEMENT,
            format!("time zone displacement out of range: \"{s}\""),
        ),
    }
}

/// `timestamp_in` (timestamp.c:163): the literal's zone, if any, is ignored.
pub fn timestamp_in(s: &str) -> Result<i64, Error> {
    if let Some(t) = special(s) {
        return Ok(t);
    }
    let parsed = parse(s).map_err(|e| unread(e, s, "timestamp"))?;
    valid(parsed.local).ok_or_else(|| {
        Error::new(
            sqlstate::DATETIME_FIELD_OVERFLOW,
            format!("timestamp out of range: \"{s}\""),
        )
    })
}

/// `timestamptz_in` (timestamp.c:419): the literal's zone, else the session's.
pub fn timestamptz_in(s: &str, session: &TimeZone) -> Result<i64, Error> {
    if let Some(t) = special(s) {
        return Ok(t);
    }
    let parsed = parse(s).map_err(|e| unread(e, s, "timestamp with time zone"))?;
    let utc = match parsed.zone {
        Some(east) => east.checked_neg().and_then(|w| shifted(parsed.local, w)),
        None => local_to_utc(parsed.local, session),
    };
    utc.ok_or_else(|| {
        Error::new(
            sqlstate::DATETIME_FIELD_OVERFLOW,
            format!("timestamp out of range: \"{s}\""),
        )
    })
}

/// A finite timestamp in ISO form (`EncodeDateTime`, datetime.c:4471), without its era.
fn iso(t: i64) -> Option<(String, bool)> {
    let (year, month, day) = date(t.div_euclid(DAY_USECS))?;
    let time = t.rem_euclid(DAY_USECS);
    let (secs, micros) = (time.div_euclid(USECS), time.rem_euclid(USECS));
    let (hour, rest) = (secs.div_euclid(3600), secs.rem_euclid(3600));
    let (minute, second) = (rest.div_euclid(60), rest.rem_euclid(60));
    let bc = year <= 0;
    let shown = if bc { 1_i64.checked_sub(year)? } else { year };
    let mut s = format!("{shown:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}");
    if micros != 0 {
        s.push('.');
        s.push_str(format!("{micros:06}").trim_end_matches('0'));
    }
    Some((s, bc))
}

fn infinite(t: i64) -> Option<String> {
    match t {
        i64::MAX => Some("infinity".to_owned()),
        i64::MIN => Some("-infinity".to_owned()),
        _ => None,
    }
}

/// `timestamp_out` (timestamp.c:236) in DateStyle ISO.
pub fn timestamp_out(t: i64) -> Result<String, Error> {
    if let Some(s) = infinite(t) {
        return Ok(s);
    }
    let (mut s, bc) = iso(t).ok_or_else(out_of_range)?;
    if bc {
        s.push_str(" BC");
    }
    Ok(s)
}

/// `timestamptz_out` (timestamp.c:793) in DateStyle ISO: the zone's clock, then its offset
/// (`EncodeTimezone`, datetime.c:4354).
pub fn timestamptz_out(t: i64, zone: &TimeZone) -> Result<String, Error> {
    if let Some(s) = infinite(t) {
        return Ok(s);
    }
    let east = unix_second(t)
        .map(|s| zone.at_utc(s))
        .ok_or_else(out_of_range)?;
    let (mut s, bc) = i64::from(east)
        .checked_mul(USECS)
        .and_then(|e| t.checked_add(e))
        .and_then(iso)
        .ok_or_else(out_of_range)?;
    let offset = east.unsigned_abs();
    let (hours, rest) = (offset.div_euclid(3600), offset.rem_euclid(3600));
    let (minutes, seconds) = (rest.div_euclid(60), rest.rem_euclid(60));
    s.push(if east < 0 { '-' } else { '+' });
    s.push_str(&format!("{hours:02}"));
    if minutes != 0 || seconds != 0 {
        s.push_str(&format!(":{minutes:02}"));
    }
    if seconds != 0 {
        s.push_str(&format!(":{seconds:02}"));
    }
    if bc {
        s.push_str(" BC");
    }
    Ok(s)
}
