#!/usr/bin/env python3
"""Lane V19's trace (never for main): logs, in test builds, each stage of a replica log's restart and of a local
read, to stderr, with the microseconds since the process's first line. Applied to a clone by path."""
import sys, pathlib, re
root = pathlib.Path(sys.argv[1])

def sub(path, old, new, count=1):
    p = root / path
    s = p.read_text()
    n = s.count(old)
    if n < count:
        raise SystemExit(f"{path}: expected at least {count} of {old[:60]!r}, found {n}")
    p.write_text(s.replace(old, new, count))

# 1. the helper, in lib.rs
sub("crates/amphora/src/lib.rs", "pub mod audit;", '''#[cfg(test)]
pub(crate) fn v19(line: &str) {
    use std::sync::OnceLock;
    static T0: OnceLock<std::time::Instant> = OnceLock::new();
    let t = T0.get_or_init(std::time::Instant::now).elapsed().as_micros();
    let thread = std::thread::current();
    eprintln!("V19 {t:>9}us [{}] {line}", thread.name().unwrap_or("?"));
}
#[cfg(test)]
pub(crate) fn v19_once(key: &str, line: &str) {
    use std::sync::{Mutex, OnceLock};
    static LAST: OnceLock<Mutex<std::collections::HashMap<String, String>>> = OnceLock::new();
    let last = LAST.get_or_init(Mutex::default);
    let mut last = last.lock().unwrap();
    if last.get(key).is_some_and(|seen| seen == line) {
        return;
    }
    last.insert(key.to_string(), line.to_string());
    drop(last);
    v19(&format!("{key} {line}"));
}

pub mod audit;''')

R = "crates/amphora/src/database/replicated.rs"
# 2. with(): waits
sub(R, '''            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                self.meter.local_refused(LocalRefusal::Timeout);''', '''            let left = until.saturating_duration_since(Instant::now());
            #[cfg(test)]
            crate::v19_once(
                &format!("with id={}", self.id),
                &format!(
                    "waits held={held} out={:?} idle={} left_s={}",
                    ledger.out, ledger.idle.len(), left.as_secs()
                ),
            );
            if left.is_zero() {
                self.meter.local_refused(LocalRefusal::Timeout);''')
sub(R, '''        self.meter.local_read();
        let answer = f(&mut reader).map_err(|error| refused(&error));
        self.give_back(Some((reader, charge)));''', '''        self.meter.local_read();
        #[cfg(test)]
        crate::v19(&format!("with id={} reads", self.id));
        let answer = f(&mut reader).map_err(|error| refused(&error));
        #[cfg(test)]
        crate::v19(&format!("with id={} gives back", self.id));
        self.give_back(Some((reader, charge)));''')
# 3. hold/unhold/restart_unread
sub(R, '''        locked(&self.account.ledger)?.restarting.insert(self.id);
        Ok(())''', '''        let _new = locked(&self.account.ledger)?.restarting.insert(self.id);
        #[cfg(test)]
        if _new {
            crate::v19(&format!("hold_reads id={}", self.id));
        }
        Ok(())''')
sub(R, '''        if locked(&self.account.ledger)?.restarting.remove(&self.id) {
            self.account.free();
        }
        Ok(())''', '''        if locked(&self.account.ledger)?.restarting.remove(&self.id) {
            #[cfg(test)]
            crate::v19(&format!("unhold_reads id={}", self.id));
            self.account.free();
        }
        Ok(())''')
sub(R, '''        if locked(&self.account.ledger)?.out.contains_key(&self.id) {
            return Ok(None);
        }''', '''        if locked(&self.account.ledger)?.out.contains_key(&self.id) {
            #[cfg(test)]
            crate::v19_once(
                &format!("restart_unread id={}", self.id),
                "a read is out: none",
            );
            return Ok(None);
        }''')
sub(R, '''        let restarted = restart().map_err(|error| refused(&error))?;
        if restarted {''', '''        let restarted = restart().map_err(|error| refused(&error))?;
        #[cfg(test)]
        crate::v19_once(
            &format!("restart_unread id={}", self.id),
            &format!("restart_log -> {restarted}"),
        );
        if restarted {''')
# 4. the owner's flush
sub(R, '''                let due = past || (frames > 0 && self.spacing.due()?);
                if hold_due(pass, due, past) && !lent {''', '''                let due = past || (frames > 0 && self.spacing.due()?);
                #[cfg(test)]
                crate::v19_once(
                    &format!("owner id={}", self.control.local.id),
                    &format!(
                        "flush frames={frames} room={} past={past} due={due} out={} caught_up={} busy={} lent={lent} applied={} flushed_backlog={} segs={:?}",
                        pass.room,
                        *out,
                        pass.caught_up,
                        pass.busy,
                        self.replica.machine().applied().index,
                        self.replica.machine().flush_backlog(),
                        self.replica.machine().v19_segments(),
                    ),
                );
                if hold_due(pass, due, past) && !lent {''')
sub(R, '''                    ask.try_send(Ask { at, copy: due })''', '''                    #[cfg(test)]
                    crate::v19(&format!(
                        "owner id={} asks at={} copy={due}",
                        self.control.local.id, at.index
                    ));
                    ask.try_send(Ask { at, copy: due })''')
# 5. the flushing thread
sub(R, '''                        let began = Instant::now();
                        copied_at = Some(began);
                        let said = match copier.checkpoint() {''', '''                        let began = Instant::now();
                        copied_at = Some(began);
                        #[cfg(test)]
                        crate::v19(&format!("flusher copies (copy={copy})"));
                        let said = match copier.checkpoint() {''')
sub(R, '''                            Ok(Pass {
                                caught_up,
                                busy: copied.is_none(),''', '''                            #[cfg(test)]
                            crate::v19(&format!(
                                "flusher pass copied={copied:?} through={through:?} caught_up={caught_up} room={room}"
                            ));
                            Ok(Pass {
                                caught_up,
                                busy: copied.is_none(),''')
# 6. the machine: segments
sub("crates/amphora/src/replication.rs", '''    pub(crate) fn restart_log(&mut self) -> anyhow::Result<bool> {
        self.db.restart_log()
    }''', '''    pub(crate) fn restart_log(&mut self) -> anyhow::Result<bool> {
        self.db.restart_log()
    }

    #[cfg(test)]
    pub(crate) fn v19_segments(&self) -> String {
        self.db.v19_segments()
    }''')
sub("crates/amphora/src/db.rs", '''    pub fn restart_log(&mut self) -> anyhow::Result<bool> {''', '''    #[cfg(test)]
    pub(crate) fn v19_segments(&self) -> String {
        format!(
            "{} backfill={:?} end={:?}",
            self.db.v19_segments(),
            self.db.wal_backfill(),
            self.db.wal_end()
        )
    }

    pub fn restart_log(&mut self) -> anyhow::Result<bool> {''')
sub("crates/engine/database.rs", '''    pub fn wal_segment(&self) -> storage::log_segments::SegmentId {''', '''    pub fn v19_segments(&self) -> String {
        format!("{:?}", self.shared_wal.segments.load())
    }

    pub fn wal_segment(&self) -> storage::log_segments::SegmentId {''')
print("ok")

# 7. the tests' markers
sub(R, '''        let restarts = handle.counters().database_log_restarts;
        let (first, second, frames, room, held_off, waited) = learner''', '''        let restarts = handle.counters().database_log_restarts;
        crate::v19(&format!("TEST1 settled: restarts={restarts} limit={} log={} {}", handle.counters().database_log_limit_bytes, handle.counters().database_log_bytes, driven.owner(1).replica.machine().v19_segments()));
        let (first, second, frames, room, held_off, waited) = learner''')
sub(R, '''        let passed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        std::thread::scope(|scope| {''', '''        crate::v19(&format!("TEST2 settled: restarts={restarts} limit={} log={} {}", handle.counters().database_log_limit_bytes, handle.counters().database_log_bytes, driven.owner(1).replica.machine().v19_segments()));
        let passed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        std::thread::scope(|scope| {''')
print("markers ok")
