//! The settings a wait for a row obeys, `deadlock_timeout` and `lock_timeout` (REL_19_0
//! src/backend/utils/misc/guc_parameters.dat), their values read and shown as guc.c's `parse_int` and
//! `ShowGUCOption` do, and the values a transaction that rolls back undoes.

use std::time::Duration;

use super::session::WaitLimits;
use super::wire::{PgError, SqlState};

/// A session's values, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    deadlock_timeout: i32,
    lock_timeout: i32,
}

/// The values 19 boots with.
impl Default for Settings {
    fn default() -> Self {
        Self {
            deadlock_timeout: 1000,
            lock_timeout: 0,
        }
    }
}

/// One of them: its least value (the greatest is `INT_MAX`), and whether only a superuser sets it
/// (`PGC_SUSET`).
struct Setting {
    name: &'static str,
    least: i32,
    superuser: bool,
    value: fn(&mut Settings) -> &mut i32,
}

const SETTINGS: [Setting; 2] = [
    Setting {
        name: "deadlock_timeout",
        least: 1,
        superuser: true,
        value: |s| &mut s.deadlock_timeout,
    },
    Setting {
        name: "lock_timeout",
        least: 0,
        superuser: false,
        value: |s| &mut s.lock_timeout,
    },
];

/// `convert_to_base_unit`'s time units (guc.c's `time_unit_conversion_table`) in milliseconds, largest
/// first.
const UNITS: [(&str, f64); 6] = [
    ("d", 86_400_000.0),
    ("h", 3_600_000.0),
    ("min", 60_000.0),
    ("s", 1000.0),
    ("ms", 1.0),
    ("us", 0.001),
];

impl Settings {
    fn setting(name: &str) -> Option<&'static Setting> {
        SETTINGS.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// Whether `name` is one of them (`guc_name_compare` folds its case).
    #[must_use]
    pub fn has(name: &str) -> bool {
        Self::setting(name).is_some()
    }

    fn known(name: &str) -> Result<&'static Setting, PgError> {
        Self::setting(name).ok_or_else(|| {
            PgError::error(
                SqlState::new("42704"),
                format!("unrecognized configuration parameter \"{name}\""),
            )
        })
    }

    /// These with `name` as `value` has it, none for what 19 boots with.
    pub fn with(mut self, name: &str, value: Option<&str>, superuser: bool) -> Result<Self, PgError> {
        let setting = Self::known(name)?;
        if setting.superuser && !superuser {
            return Err(PgError::error(
                SqlState::new("42501"),
                format!("permission denied to set parameter \"{}\"", setting.name),
            ));
        }
        *(setting.value)(&mut self) = match value {
            Some(text) => milliseconds(setting, text)?,
            None => *(setting.value)(&mut Self::default()),
        };
        Ok(self)
    }

    /// `name`'s canonical name and its value as SHOW gives it: in the largest unit that holds it
    /// exactly (`convert_int_from_base_unit`).
    pub fn show(mut self, name: &str) -> Result<(&'static str, String), PgError> {
        let setting = Self::known(name)?;
        let value = *(setting.value)(&mut self);
        let shown = UNITS
            .iter()
            .filter(|(_, size)| *size >= 1.0)
            .find(|(_, size)| value > 0 && f64::from(value) % size == 0.0)
            .map_or_else(
                || value.to_string(),
                |(unit, size)| format!("{}{unit}", f64::from(value) / size),
            );
        Ok((setting.name, shown))
    }

    /// The deadlines of a wait that begins now.
    #[must_use]
    pub fn wait_limits(self) -> WaitLimits {
        let span = |ms: i32| Duration::from_millis(u64::try_from(ms).unwrap_or(0));
        WaitLimits {
            deadlock: span(self.deadlock_timeout),
            lock: (self.lock_timeout > 0).then(|| span(self.lock_timeout)),
        }
    }
}

/// `parse_int` of a `GUC_UNIT_MS` value (guc.c): a number, a fraction rounded to even, and a unit if
/// any; then the range `parse_and_validate_value` holds it to.
fn milliseconds(setting: &Setting, text: &str) -> Result<i32, PgError> {
    let trimmed = text.trim();
    let at = trimmed
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(at);
    let size = match unit.trim_end() {
        "" => Some(1.0),
        unit => UNITS
            .iter()
            .find(|(name, _)| *name == unit)
            .map(|(_, size)| *size),
    };
    let invalid = || {
        PgError::error(
            SqlState::new("22023"),
            format!(
                "invalid value for parameter \"{}\": \"{text}\"",
                setting.name
            ),
        )
    };
    let Ok(number) = number.trim_end().parse::<f64>() else {
        return Err(invalid());
    };
    let Some(size) = size else {
        return Err(invalid().with_hint(
            "Valid units for this parameter are \"us\", \"ms\", \"s\", \"min\", \"h\", and \"d\".",
        ));
    };
    let value = (number * size).round_ties_even();
    if value.is_nan() || value > f64::from(i32::MAX) || value < f64::from(i32::MIN) {
        return Err(invalid().with_hint("Value exceeds integer range."));
    }
    if value < f64::from(setting.least) {
        return Err(PgError::error(
            SqlState::new("22023"),
            format!(
                "{value} ms is outside the valid range for parameter \"{}\" ({} ms .. {} ms)",
                setting.name,
                setting.least,
                i32::MAX
            ),
        ));
    }
    // Within i32's range, and whole.
    format!("{value}").parse().map_err(|_| invalid())
}

/// A session's value that a transaction may change: what it is now, what it becomes if the transaction
/// commits (`SET LOCAL` changes only the first), and what it goes back to if it doesn't (guc.c's
/// `AtEOXact_GUC`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Transactional<T> {
    current: T,
    session: T,
    committed: T,
}

impl<T: Copy> Transactional<T> {
    pub fn current(&self) -> T {
        self.current
    }

    pub fn session(&self) -> T {
        self.session
    }

    /// Out of a block: for good.
    pub fn set(&mut self, value: T) {
        self.current = value;
        self.session = value;
        self.committed = value;
    }

    pub fn set_in_block(&mut self, current: T, session: T) {
        self.current = current;
        self.session = session;
    }

    pub fn set_local(&mut self, value: T) {
        self.current = value;
    }

    pub fn commit(&mut self) {
        self.committed = self.session;
        self.current = self.session;
    }

    pub fn rollback(&mut self) {
        self.current = self.committed;
        self.session = self.committed;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// T-PG.15.3: a role that is no superuser may not set `deadlock_timeout` (`PGC_SUSET`), and may set
    /// `lock_timeout`.
    #[test]
    fn t_pg_15_3_only_a_superuser_sets_deadlock_timeout() {
        let set = |name, superuser| Settings::default().with(name, Some("5s"), superuser);
        let refused = set("deadlock_timeout", false).unwrap_err();
        assert_eq!(refused.code.code(), "42501");
        assert_eq!(
            refused.message,
            "permission denied to set parameter \"deadlock_timeout\""
        );
        let set = set("lock_timeout", false).unwrap();
        assert_eq!(set.wait_limits().lock, Some(Duration::from_secs(5)));
        assert!(Settings::default().with("deadlock_timeout", Some("5s"), true).is_ok());
    }
}
