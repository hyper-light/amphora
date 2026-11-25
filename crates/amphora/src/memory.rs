//! The memory this process may use, as the system says, and the budget that shares it out
//! (docs/design/store.md, decisions 26 and "Decision (lane constants): one process memory budget";
//! MB.1). The limit is the smallest of: the memory limits of the control groups the process runs in
//! (cgroup v2, else v1) and the machine's own memory on Linux; the machine's physical memory on
//! macOS (`hw.memsize`) and Windows (`GlobalMemoryStatusEx`); and, on Unix, the process's own soft
//! address-space, data and resident-set limits (`getrlimit`). Where none of them can be read the
//! budget refuses, typed, naming the platform: no fallback is picked.
//!
//! The parsers are pure, so tests give them each file's text; [`limit`] reads the files.

use amphora_api::Refusal;
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicU64, Ordering, fence};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::Instant;

/// A cgroup v1 limit at or past this is "no limit" (the kernel reports its page-rounded
/// `LONG_MAX`, 9,223,372,036,854,771,712 on 4 KiB pages).
const V1_UNLIMITED: u64 = 1 << 62;

/// The path of this process's cgroup v2 group, from `/proc/self/cgroup`'s `0::` line.
#[must_use]
pub fn v2_group(proc_self_cgroup: &str) -> Option<&str> {
    proc_self_cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .map(str::trim)
}

/// The path of this process's cgroup v1 memory group, from `/proc/self/cgroup`'s line whose
/// controllers include `memory`.
#[must_use]
pub fn v1_group(proc_self_cgroup: &str) -> Option<&str> {
    proc_self_cgroup.lines().find_map(|line| {
        let mut fields = line.splitn(3, ':');
        let (_, controllers, path) = (fields.next()?, fields.next()?, fields.next()?);
        controllers
            .split(',')
            .any(|c| c == "memory")
            .then(|| path.trim())
    })
}

/// A cgroup's memory limit in bytes from its `memory.max` (v2) or `memory.limit_in_bytes` (v1):
/// None for "max" and for v1's no-limit value.
#[must_use]
pub fn group_limit(text: &str) -> Option<u64> {
    read_group_limit(text).ok().flatten()
}

fn read_group_limit(text: &str) -> Result<Option<u64>, std::num::ParseIntError> {
    let text = text.trim();
    if text == "max" {
        return Ok(None);
    }
    let bytes = text.parse::<u64>()?;
    if bytes >= V1_UNLIMITED {
        return Ok(None);
    }
    Ok(Some(bytes))
}

/// The machine's memory in bytes, from `/proc/meminfo`'s `MemTotal:` line (in KiB).
#[must_use]
pub fn mem_total(meminfo: &str) -> Option<u64> {
    let line = meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))?;
    let kib = line.split_whitespace().next()?.parse::<u64>().ok()?;
    kib.checked_mul(1024)
}

/// Each group from `path` up to the hierarchy's root, deepest first: `/a/b` gives `/a/b`, `/a` and
/// `/` (a limit anywhere above a group bounds it too).
#[must_use]
pub fn ancestors(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = path.trim_end_matches('/');
    while !at.is_empty() {
        out.push(at.to_owned());
        at = match at.rfind('/') {
            Some(0) | None => "",
            Some(cut) => at.get(..cut).unwrap_or_default(),
        };
    }
    out.push("/".to_owned());
    out
}

fn min_opt(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

/// The memory this process may use, in bytes: the smallest of what the platform says (the module's
/// head lists the readings). None where nothing can be read.
#[must_use]
pub fn limit() -> Option<u64> {
    min_opt(platform_limit(), process_limit())
}

/// Linux: the limits of every control group above this process (v2 and v1) and the machine's
/// memory (`/proc/meminfo`).
#[cfg(target_os = "linux")]
fn platform_limit() -> Option<u64> {
    let read = |path: &str| std::fs::read_to_string(path).ok();
    let cgroup = read("/proc/self/cgroup").unwrap_or_default();
    let mut best: Option<u64> = None;
    if let Some(group) = v2_group(&cgroup) {
        for at in ancestors(group) {
            let dir = format!("/sys/fs/cgroup{}", if at == "/" { "" } else { at.as_str() });
            if let Some(n) = read(&format!("{dir}/memory.max"))
                .as_deref()
                .and_then(group_limit)
            {
                best = min_opt(best, Some(n));
            }
        }
    }
    if let Some(group) = v1_group(&cgroup) {
        for at in ancestors(group) {
            let dir = format!(
                "/sys/fs/cgroup/memory{}",
                if at == "/" { "" } else { at.as_str() }
            );
            if let Some(n) = read(&format!("{dir}/memory.limit_in_bytes"))
                .as_deref()
                .and_then(group_limit)
            {
                best = min_opt(best, Some(n));
            }
        }
    }
    if let Some(n) = read("/proc/meminfo").as_deref().and_then(mem_total) {
        best = min_opt(best, Some(n));
    }
    best
}

/// macOS and Windows: the machine's physical memory as the platform reports it (the engine makes
/// the call: this crate denies `unsafe`). Physical RAM does not change during a process's life,
/// so read it once instead of invoking `sysctl`/`GlobalMemoryStatusEx` on every budget check. A
/// Windows job object's memory limit isn't read (it needs `QueryInformationJobObject`): the
/// machine's memory bounds such a process from above.
#[cfg(not(target_os = "linux"))]
fn platform_limit() -> Option<u64> {
    static PHYSICAL: OnceLock<Option<u64>> = OnceLock::new();
    *PHYSICAL.get_or_init(amphora_engine::io::system_memory::physical_memory)
}

/// The process's own soft limits on what it may map (POSIX `getrlimit`: `RLIMIT_AS`,
/// `RLIMIT_DATA`, and `RLIMIT_RSS` where the system enforces it), each where one is set.
#[cfg(unix)]
fn process_limit() -> Option<u64> {
    use rustix::process::{Resource, getrlimit};
    [Resource::As, Resource::Data, Resource::Rss]
        .into_iter()
        .filter_map(|resource| getrlimit(resource).current)
        .min()
}

/// Windows has no per-process soft limit of this kind (its job objects aside, above).
#[cfg(windows)]
fn process_limit() -> Option<u64> {
    None
}

/// The bytes this process holds resident now (Linux `/proc/self/statm`, macOS `task_info`,
/// Windows `GetProcessMemoryInfo`, through memory-stats). None where it can't be read.
#[must_use]
pub fn resident() -> Option<u64> {
    memory_stats::memory_stats().and_then(|stats| u64::try_from(stats.physical_mem).ok())
}

/// What the process's memory is shared out to, each by its own measured use (MB.1): the owners'
/// queues (commands waiting, messages going out), the images a replica makes and installs, the
/// caches (the page pool and what else keeps data to read again), and the chunks of a database's
/// file a backup copies and a restore fetches (T-T1.12.M1), and the PostgreSQL sessions, each its
/// state and the frame it holds (docs/design/postgres.md, decision 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Use {
    Queues,
    Images,
    Caches,
    Chunks,
    Sessions,
}

impl Use {
    #[cfg(test)]
    const ALL: [Self; 5] = [
        Self::Queues,
        Self::Images,
        Self::Caches,
        Self::Chunks,
        Self::Sessions,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Queues => "the owners' queues",
            Self::Images => "an image",
            Self::Caches => "the caches",
            Self::Chunks => "a backup's chunk",
            Self::Sessions => "the PostgreSQL sessions",
        }
    }
}

/// One reading of what the budget is learned from: the limit and the process's resident bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
    pub limit: Option<u64>,
    pub resident: Option<u64>,
}

/// Where a budget reads from: the system's own readings, or a test's fed ones.
pub trait Readings: Send + Sync {
    fn read(&self) -> Reading;
}

/// Strictly linearizable, zero-`Arc`, zero-mutex single-flight coalescer for [`Readings`]
/// (`AC-MB.1.1`).
///
/// Every caller receives a fresh reading whose `R::read()` **started after** the caller entered
/// `SingleFlight::read()` and **finished before** it returned. When a reading is already in flight,
/// arriving callers never reuse the previous completed sample (which could predate a memory limit
/// reduction); instead they coalesce onto the next wave executed by one elected leader once the
/// current wave finishes.
///
/// All state lives inline in `SingleFlight<R>` and is accessed solely through `&self` (zero `Arc`,
/// zero heap allocation, zero `Mutex`, zero `Condvar`, zero `unsafe`):
/// - Two alternating [`amphora_engine::FastRwLock<()>`] phases (`phases[epoch & 1]`) coordinate
///   sleeping and parallel fan-out waking: a wave leader acquires `phases[epoch & 1].write()`
///   *before* publishing its state, and all followers park on `phases[epoch & 1].read()` so every
///   follower wakes in parallel once the write guard drops.
/// - A 4-mode monotonic generation word (`state = (epoch << 2) | mode`) prevents both fast-reader
///   starvation and lock-order inversions: when wave `g` finishes with wave `g + 1` queued, wave
///   `g` transitions to `MODE_HANDING_OFF(g)` and drops `phases[g & 1].write()`. No wave `g + 2`
///   can claim `phases[g & 1].write()` until wave `g + 1`'s leader has woken, dropped its
///   `phases[g & 1].read()`, and published `MODE_ACTIVE(g + 1)` — sealing wave `g + 1` before
///   calling `inner.read()`. Callers arriving during `MODE_HANDING_OFF(g)` coalesce onto wave
///   `g + 1` as followers.
/// - Completed readings are published through a single-writer lock-free seqlock
///   ([`LatestReading`]) tagged with the full 62-bit wave generation, eliminating mod-2 parity
///   aliasing. If a leader panics inside `inner.read()`, its drop guard advances `state` without
///   publishing a reading and drops its write guard, waking every waiting follower and next-wave
///   leader cleanly.
const MODE_MASK: u64 = 0b11;
const MODE_IDLE: u64 = 0;
const MODE_ACTIVE: u64 = 1;
const MODE_QUEUED: u64 = 2;
const MODE_HANDING_OFF: u64 = 3;

#[inline]
const fn pack_state(epoch: u64, mode: u64) -> u64 {
    (epoch << 2) | (mode & MODE_MASK)
}

#[inline]
const fn state_epoch(packed: u64) -> u64 {
    packed >> 2
}

#[inline]
const fn state_mode(packed: u64) -> u64 {
    packed & MODE_MASK
}

struct LatestReading {
    /// Even when stable, odd while the unique active flight leader is updating the fields below.
    seq: AtomicU64,
    epoch: AtomicU64,
    has_limit: AtomicU64,
    limit: AtomicU64,
    has_resident: AtomicU64,
    resident: AtomicU64,
}

impl LatestReading {
    const fn new() -> Self {
        Self {
            seq: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
            has_limit: AtomicU64::new(0),
            limit: AtomicU64::new(0),
            has_resident: AtomicU64::new(0),
            resident: AtomicU64::new(0),
        }
    }

    /// Publishes `(epoch, reading)`. Called exclusively by the active flight leader while it holds
    /// its wave's write guard, so there is at most one writer at a time.
    fn publish(&self, epoch: u64, reading: Reading) {
        let s = self.seq.load(Ordering::Relaxed);
        self.seq.store(s.wrapping_add(1), Ordering::Release);
        fence(Ordering::SeqCst);
        self.epoch.store(epoch, Ordering::Relaxed);
        self.has_limit
            .store(u64::from(reading.limit.is_some()), Ordering::Relaxed);
        self.limit
            .store(reading.limit.unwrap_or(0), Ordering::Relaxed);
        self.has_resident
            .store(u64::from(reading.resident.is_some()), Ordering::Relaxed);
        self.resident
            .store(reading.resident.unwrap_or(0), Ordering::Relaxed);
        self.seq.store(s.wrapping_add(2), Ordering::Release);
    }

    /// Lock-free read of the latest completed reading if its wave generation reached `target`.
    fn read_at_least(&self, target: u64) -> Option<Reading> {
        loop {
            let s1 = self.seq.load(Ordering::Acquire);
            if s1 & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let epoch = self.epoch.load(Ordering::Relaxed);
            let has_limit = self.has_limit.load(Ordering::Relaxed);
            let limit = self.limit.load(Ordering::Relaxed);
            let has_resident = self.has_resident.load(Ordering::Relaxed);
            let resident = self.resident.load(Ordering::Relaxed);
            fence(Ordering::Acquire);
            let s2 = self.seq.load(Ordering::Relaxed);
            if s1 == s2 {
                return if epoch >= target {
                    Some(Reading {
                        limit: if has_limit != 0 { Some(limit) } else { None },
                        resident: if has_resident != 0 {
                            Some(resident)
                        } else {
                            None
                        },
                    })
                } else {
                    None
                };
            }
        }
    }
}

struct SingleFlight<R> {
    inner: R,
    state: AtomicU64,
    phases: [amphora_engine::FastRwLock<()>; 2],
    latest: LatestReading,
    #[cfg(test)]
    waiters: AtomicUsize,
}

impl<R> SingleFlight<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            state: AtomicU64::new(pack_state(0, MODE_IDLE)),
            phases: [
                amphora_engine::FastRwLock::new(()),
                amphora_engine::FastRwLock::new(()),
            ],
            latest: LatestReading::new(),
            #[cfg(test)]
            waiters: AtomicUsize::new(0),
        }
    }

    #[inline]
    fn phase(&self, epoch: u64) -> &amphora_engine::FastRwLock<()> {
        let [even, odd] = &self.phases;
        if epoch & 1 == 0 { even } else { odd }
    }

    /// Transitions `state` at the end of wave `epoch` while the caller still holds
    /// `phases[epoch & 1].write()`, so any newly woken follower or next-wave leader sees the final
    /// state as soon as the write guard drops.
    fn release_wave(&self, epoch: u64) {
        if self
            .state
            .compare_exchange(
                pack_state(epoch, MODE_ACTIVE),
                pack_state(epoch, MODE_IDLE),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            // A leader for wave `epoch + 1` is already queued (holding
            // `phases[(epoch + 1) & 1].write()` and waiting on `phases[epoch & 1].read()`). Hand
            // off directly without exposing `IDLE`.
            self.state
                .store(pack_state(epoch, MODE_HANDING_OFF), Ordering::Release);
        }
    }

    /// Executes `inner.read()` as the unique leader of wave `epoch`, holding `write_guard` until
    /// `latest` and `state` are updated (or, on panic, until `state` has been transitioned so
    /// waiting followers and any queued next leader wake cleanly).
    fn run_wave(&self, epoch: u64, write_guard: impl Drop) -> Reading
    where
        R: Readings,
    {
        struct ActiveGuard<'a, R> {
            sf: &'a SingleFlight<R>,
            epoch: u64,
            finished: bool,
        }
        impl<R> Drop for ActiveGuard<'_, R> {
            fn drop(&mut self) {
                if !self.finished {
                    self.sf.release_wave(self.epoch);
                }
            }
        }
        // `write_guard` is declared *before* `guard`, so on both normal return and unwind `guard`
        // drops FIRST (updating `state`) and `write_guard` drops SECOND (waking followers and any
        // queued next-wave leader).
        let _write_guard = write_guard;
        let mut guard = ActiveGuard {
            sf: self,
            epoch,
            finished: false,
        };
        let reading = self.inner.read();
        guard.finished = true;
        self.latest.publish(epoch, reading);
        self.release_wave(epoch);
        reading
    }

    #[cfg(test)]
    fn is_idle(&self) -> bool {
        state_mode(self.state.load(Ordering::Acquire)) == MODE_IDLE
            && self.waiters.load(Ordering::Acquire) == 0
    }
}

impl<R: Readings> Readings for SingleFlight<R> {
    fn read(&self) -> Reading {
        #[cfg(test)]
        struct WaiterGuard<'a>(&'a AtomicUsize);
        #[cfg(test)]
        impl Drop for WaiterGuard<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::AcqRel);
            }
        }

        loop {
            let cur = self.state.load(Ordering::SeqCst);
            let epoch = state_epoch(cur);
            match state_mode(cur) {
                MODE_IDLE => {
                    let next = epoch.wrapping_add(1);
                    if let Some(write_guard) = self.phase(next).try_write() {
                        if self
                            .state
                            .compare_exchange(
                                cur,
                                pack_state(next, MODE_ACTIVE),
                                Ordering::SeqCst,
                                Ordering::Acquire,
                            )
                            .is_ok()
                        {
                            return self.run_wave(next, write_guard);
                        }
                        drop(write_guard);
                        // Another thread won `next` strictly after our `SeqCst` load of `IDLE`;
                        // if it already finished, its reading is linearizable for us.
                        if let Some(reading) = self.latest.read_at_least(next) {
                            return reading;
                        }
                        continue;
                    }
                    // Either a peer just grabbed `phases[next & 1].write()` for wave `next` or a
                    // straggling follower of `epoch - 1` is mid-drop of `read()`. If wave `next`
                    // is already active, coalesce onto it as a follower without spinning.
                    if self.state.load(Ordering::SeqCst) == pack_state(next, MODE_ACTIVE) {
                        {
                            #[cfg(test)]
                            let _waiter = {
                                self.waiters.fetch_add(1, Ordering::AcqRel);
                                WaiterGuard(&self.waiters)
                            };
                            drop(self.phase(next).read());
                        }
                        if let Some(reading) = self.latest.read_at_least(next) {
                            return reading;
                        }
                        continue;
                    }
                    std::thread::yield_now();
                }
                MODE_ACTIVE => {
                    let next = epoch.wrapping_add(1);
                    if let Some(next_guard) = self.phase(next).try_write() {
                        if self
                            .state
                            .compare_exchange(
                                cur,
                                pack_state(epoch, MODE_QUEUED),
                                Ordering::SeqCst,
                                Ordering::Acquire,
                            )
                            .is_ok()
                        {
                            // We are the sole leader of wave `next`, already holding
                            // `phases[next & 1].write()`. Wait for wave `epoch` to drop
                            // `phases[epoch & 1].write()`, release our read guard, then seal wave
                            // `next` (`MODE_ACTIVE(next)`) before invoking `inner.read()`.
                            {
                                #[cfg(test)]
                                let _waiter = {
                                    self.waiters.fetch_add(1, Ordering::AcqRel);
                                    WaiterGuard(&self.waiters)
                                };
                                drop(self.phase(epoch).read());
                            }
                            self.state
                                .store(pack_state(next, MODE_ACTIVE), Ordering::SeqCst);
                            return self.run_wave(next, next_guard);
                        }
                        drop(next_guard);
                        continue;
                    }
                    // Another contender already holds `phases[next & 1].write()` (or a straggler
                    // from `epoch - 1` is dropping `read()`). Re-check state; if `QUEUED` or
                    // `HANDING_OFF`, the next loop iteration parks as a shared follower.
                    if state_mode(self.state.load(Ordering::Acquire)) == MODE_ACTIVE {
                        std::thread::yield_now();
                    }
                }
                MODE_QUEUED | MODE_HANDING_OFF => {
                    // Wave `next` has a leader holding `phases[next & 1].write()` and has NOT
                    // yet sealed `MODE_ACTIVE(next)` or started `inner.read()`. Park as a shared
                    // follower on `phases[next & 1].read()` (zero write contention!).
                    let next = epoch.wrapping_add(1);
                    {
                        #[cfg(test)]
                        let _waiter = {
                            self.waiters.fetch_add(1, Ordering::AcqRel);
                            WaiterGuard(&self.waiters)
                        };
                        drop(self.phase(next).read());
                    }
                    if let Some(reading) = self.latest.read_at_least(next) {
                        return reading;
                    }
                    // Wave `next`'s leader panicked before publishing; retry cleanly.
                }
                _ => {}
            }
        }
    }
}

/// The system's raw kernel readings, taken afresh on every call: a host may change a limit while
/// the process runs. On Linux each file is opened once and read again in place at every ask
/// ([`Opened`]): the same readings, exact at each ask, without the opens, path walks and
/// `/proc/meminfo` they cost.
struct RawSystem {
    #[cfg(target_os = "linux")]
    opened: std::sync::RwLock<Option<Arc<Opened>>>,
}

impl RawSystem {
    fn new() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            opened: std::sync::RwLock::new(None),
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn read_fresh(&self) -> Reading {
        Reading {
            limit: limit(),
            resident: resident(),
        }
    }

    #[cfg(target_os = "linux")]
    fn read_fresh(&self) -> Reading {
        let held = self.opened.read().ok().and_then(|o| o.clone());
        if let Some(reading) = held.as_deref().and_then(Opened::reading) {
            return reading;
        }
        let fresh = Opened::open().map(Arc::new);
        let reading = fresh.as_deref().and_then(Opened::reading);
        if let Ok(mut slot) = self.opened.write() {
            *slot = fresh.filter(|_| reading.is_some());
        }
        reading.unwrap_or_else(|| Reading {
            limit: limit(),
            resident: resident(),
        })
    }
}

impl Readings for RawSystem {
    fn read(&self) -> Reading {
        self.read_fresh()
    }
}

/// The system's readings, strictly linearizable (`AC-MB.1.1`) via [`SingleFlight`] so concurrent
/// callers coalesce onto a reading started strictly after they arrived rather than reusing an
/// obsolete pre-reduction sample.
struct System(SingleFlight<RawSystem>);

impl System {
    fn new() -> Self {
        Self(SingleFlight::new(RawSystem::new()))
    }
}

impl Readings for System {
    fn read(&self) -> Reading {
        self.0.read()
    }
}

/// Linux's reading files, open (AUDIT 2026-11-23 §1: the budget read every file again at every
/// request). `/proc/self/cgroup` is read again at every ask, and the groups' limit files opened
/// anew when it changed (the process was moved); each limit file and `/proc/self/statm` is read
/// again in place (`pread`), so every reading is the kernel's at the ask. The machine's memory is
/// `sysinfo`'s `totalram`, the count `/proc/meminfo`'s `MemTotal` prints, without the rest of that
/// file's work. Opened again in a child of a `fork`, whose `/proc/self` is its own.
#[cfg(target_os = "linux")]
struct Opened {
    pid: u32,
    cgroup: std::fs::File,
    /// `/proc/self/cgroup`'s text when `limits` were opened.
    seen: Vec<u8>,
    /// Each group's limit file above this process, v2's `memory.max` and v1's
    /// `memory.limit_in_bytes`, deepest first.
    limits: Vec<std::fs::File>,
    /// The `cgroup.controllers` of each v2 group above this process with no `memory.max`: one
    /// grows one when its parent enables the memory controller, so these files say when to open
    /// the groups' limit files again.
    unlimited: Vec<std::fs::File>,
    statm: Option<std::fs::File>,
}

/// The most of a file read in place: `/proc/self/cgroup` past it is read whole instead. Not a
/// tuning: a limit file's text is one number, statm's seven, and a cgroup file longer than this is
/// read by [`whole`].
#[cfg(target_os = "linux")]
const IN_PLACE: usize = 4096;

/// `file`'s text from its start, read in place; None where it can't be read.
#[cfg(target_os = "linux")]
fn read_at(file: &std::fs::File, buf: &mut [u8; IN_PLACE]) -> Option<usize> {
    use std::os::unix::fs::FileExt;
    let mut at = 0usize;
    loop {
        let rest = buf.get_mut(at..)?;
        if rest.is_empty() {
            return Some(at);
        }
        let n = file.read_at(rest, u64::try_from(at).ok()?).ok()?;
        if n == 0 {
            return Some(at);
        }
        at = at.checked_add(n)?;
    }
}

/// `file`'s whole text from its start.
#[cfg(target_os = "linux")]
fn whole(file: &std::fs::File) -> Option<Vec<u8>> {
    use std::os::unix::fs::FileExt;
    let mut out = Vec::new();
    let mut buf = [0u8; IN_PLACE];
    loop {
        let n = file
            .read_at(&mut buf, u64::try_from(out.len()).ok()?)
            .ok()?;
        if n == 0 {
            return Some(out);
        }
        out.extend_from_slice(buf.get(..n)?);
    }
}

#[cfg(target_os = "linux")]
impl Opened {
    fn open() -> Option<Self> {
        let cgroup = std::fs::File::open("/proc/self/cgroup").ok()?;
        Self::open_groups(cgroup, std::path::Path::new("/sys/fs/cgroup"))
    }

    fn open_groups(cgroup: std::fs::File, root: &std::path::Path) -> Option<Self> {
        let seen = whole(&cgroup)?;
        let text = String::from_utf8_lossy(&seen);
        let (mut limits, mut unlimited) = (Vec::new(), Vec::new());
        for (v2, group, root, file) in [
            (true, v2_group(&text), root.to_owned(), "memory.max"),
            (
                false,
                v1_group(&text),
                root.join("memory"),
                "memory.limit_in_bytes",
            ),
        ] {
            for at in group.map(ancestors).unwrap_or_default() {
                let dir = root.join(at.trim_start_matches('/'));
                match std::fs::File::open(dir.join(file)) {
                    Ok(open) => limits.push(open),
                    Err(error) if v2 && error.kind() == std::io::ErrorKind::NotFound => {
                        // The v2 hierarchy root has no memory.max. A child without the
                        // controller needs a readable watcher before its files can be cached.
                        if at != "/" {
                            unlimited
                                .push(std::fs::File::open(dir.join("cgroup.controllers")).ok()?);
                        }
                    }
                    Err(_) => return None,
                }
            }
        }
        Some(Self {
            pid: std::process::id(),
            cgroup,
            seen,
            limits,
            unlimited,
            statm: std::fs::File::open("/proc/self/statm").ok(),
        })
    }

    /// Whether these files are still this process's: its own (not a forked parent's), and its
    /// groups the ones they were opened for.
    fn current(&self) -> bool {
        if self.pid != std::process::id() {
            return false;
        }
        let mut buf = [0u8; IN_PLACE];
        let same = match read_at(&self.cgroup, &mut buf) {
            Some(n) if n < IN_PLACE => buf.get(..n) == Some(self.seen.as_slice()),
            _ => whole(&self.cgroup).as_deref() == Some(self.seen.as_slice()),
        };
        same && self.unlimited.iter().all(|controllers| {
            read_at(controllers, &mut buf)
                .and_then(|n| buf.get(..n))
                .is_some_and(|text| !text.split(u8::is_ascii_whitespace).any(|c| c == b"memory"))
        })
    }

    fn limit(&self) -> Option<u64> {
        let mut buf = [0u8; IN_PLACE];
        let machine = rustix::system::sysinfo();
        let machine = u64::from(machine.mem_unit.max(1)).checked_mul(machine.totalram);
        let mut limit = min_opt(machine, process_limit());
        for file in &self.limits {
            let n = read_at(file, &mut buf)?;
            let text = std::str::from_utf8(buf.get(..n)?).ok()?;
            if let Some(group) = read_group_limit(text).ok()? {
                limit = Some(limit.map_or(group, |bound| bound.min(group)));
            }
        }
        limit
    }

    /// `/proc/self/statm`'s resident pages (its second field) in bytes, as memory-stats reads it.
    fn resident(&self) -> Option<u64> {
        let mut buf = [0u8; IN_PLACE];
        let n = read_at(self.statm.as_ref()?, &mut buf)?;
        let pages = std::str::from_utf8(buf.get(..n)?)
            .ok()?
            .split_whitespace()
            .nth(1)?
            .parse::<u64>()
            .ok()?;
        pages.checked_mul(u64::try_from(rustix::param::page_size()).ok()?)
    }

    fn reading(&self) -> Option<Reading> {
        if !self.current() {
            return None;
        }
        Some(Reading {
            limit: Some(self.limit()?),
            resident: self.resident().or_else(resident),
        })
    }
}

/// The process's memory budget (MB.1): its limit, read where it runs, shared out to each [`Use`]
/// as that use's measured bytes. Each use charges what it holds as it takes it ([`Budget::take`])
/// and gives it back as it lets it go; a use's room is the limit less what everything else holds,
/// the larger of the process's resident bytes and the charges counted, its own charge given back.
/// No share is picked: a use has what the others leave, as measured now.
pub struct Budget {
    readings: Box<dyn Readings>,
    held: [AtomicU64; 5],
    /// Sum of reserved bytes across all five [`Use`] categories, updated atomically before each
    /// per-use slot on reserve and after each per-use slot on release so concurrent takes across
    /// distinct categories can never overshoot the process limit.
    total_held: AtomicU64,
    /// The streams of each use open now, each of which may hold a charge at once ([`Stream`]).
    streams: [AtomicU64; 5],
    /// The holders waiting for room now ([`Budget::take_until`]): a charge given back with none
    /// waiting takes no lock.
    waiting: AtomicU64,
    /// The charges given back while a holder waited, counted under this lock: a holder that found
    /// no room waits on it until the count moves or its deadline passes.
    given: (Mutex<u64>, Condvar),
}

/// One stream of a use's that may hold a charge at any moment while it lives (a backup's base copy,
/// a restore): counted while it lives, so each stream's room is its use's room shared by the
/// streams open now ([`Budget::stream_room`]) and every stream's charge fits together.
pub struct Stream {
    budget: Arc<Budget>,
    used: Use,
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.budget.streams_of(self.used).try_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |open| Some(open.saturating_sub(1)),
        );
    }
}

/// Bytes held against a budget by one use, given back when this drops.
pub struct Charge {
    budget: Arc<Budget>,
    used: Use,
    bytes: u64,
}

impl Charge {
    /// The bytes this charge holds.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl Drop for Charge {
    fn drop(&mut self) {
        self.budget.give(self.used, self.bytes);
    }
}

/// The engine's versioned storage takes its memory from the process's budget as caches
/// ([`Use::Caches`]: its page frames and row versions), what the rest of the process leaves it by
/// measured use (G7 step 2, `AC-NS.2.1`; MB.1). Every database the process opens shares the one
/// budget.
pub struct StorageMemory(pub Arc<Budget>);

impl amphora_engine::storage::versioned::memory::Source for StorageMemory {
    fn take(&self, what: &'static str, bytes: u64) -> amphora_engine::Result<()> {
        self.0
            .hold(Use::Caches, bytes)
            .map_err(|refusal| match refusal {
                Refusal::TooLarge { limit, .. } => {
                    amphora_engine::LimboError::MemoryBudget(Box::new(amphora_engine::BudgetFull {
                        what,
                        asked: bytes,
                        held: self.0.held(Use::Caches),
                        limit: u64::try_from(limit).unwrap_or(u64::MAX),
                    }))
                }
                other => amphora_engine::LimboError::InternalError(format!(
                    "versioned storage: {what}: {other}"
                )),
            })
    }

    fn give(&self, bytes: u64) {
        self.0.give(Use::Caches, bytes);
    }

    fn room(&self) -> amphora_engine::Result<u64> {
        self.0.room(Use::Caches).map_err(|refusal| {
            amphora_engine::LimboError::InternalError(format!(
                "versioned storage: the memory budget's room: {refusal}"
            ))
        })
    }
}

impl Budget {
    /// A budget over `readings`.
    #[must_use]
    pub fn new(readings: Box<dyn Readings>) -> Arc<Self> {
        Arc::new(Self {
            readings,
            held: Default::default(),
            total_held: AtomicU64::new(0),
            streams: Default::default(),
            waiting: AtomicU64::new(0),
            given: (Mutex::new(0), Condvar::new()),
        })
    }

    /// The counter of what `used` holds.
    fn slot(&self, used: Use) -> &AtomicU64 {
        Self::of(&self.held, used)
    }

    /// The counter of `used`'s streams open now.
    fn streams_of(&self, used: Use) -> &AtomicU64 {
        Self::of(&self.streams, used)
    }

    fn of(counters: &[AtomicU64; 5], used: Use) -> &AtomicU64 {
        let [queues, images, caches, chunks, sessions] = counters;
        match used {
            Use::Queues => queues,
            Use::Images => images,
            Use::Caches => caches,
            Use::Chunks => chunks,
            Use::Sessions => sessions,
        }
    }

    /// A stream of `used`'s opened: counted until it drops.
    #[must_use]
    pub fn stream(self: &Arc<Self>, used: Use) -> Stream {
        self.streams_of(used).fetch_add(1, Ordering::AcqRel);
        Stream {
            budget: Arc::clone(self),
            used,
        }
    }

    /// The streams of `used` open now.
    #[must_use]
    pub fn streams(&self, used: Use) -> u64 {
        self.streams_of(used).load(Ordering::Acquire)
    }

    /// One stream's room ([`Stream`]): `used`'s room ([`Budget::room`]) shared by its streams
    /// open now (the asker's own among them; one at least), so each may hold a charge that size at
    /// once.
    pub fn stream_room(&self, used: Use) -> Result<u64, Refusal> {
        Ok(self
            .room(used)?
            .checked_div(self.streams(used).max(1))
            .unwrap_or(0))
    }

    /// The bytes `used` holds now, as charged.
    #[must_use]
    pub fn held(&self, used: Use) -> u64 {
        self.slot(used).load(Ordering::Acquire)
    }

    /// The bytes `used` may hold at most now, alongside the whole process limit read.
    fn room_and_limit(&self, used: Use) -> Result<(u64, u64), Refusal> {
        let reading = self.readings.read();
        let limit = reading.limit.ok_or_else(|| unread("memory limit"))?;
        let own = self.held(used);
        let charged = self.total_held.load(Ordering::Acquire).max(own);
        let resident = reading.resident.unwrap_or(charged);
        let others = resident.max(charged).saturating_sub(own);
        Ok((limit.saturating_sub(others), limit))
    }

    /// The bytes `used` may hold at most now: the limit less what the rest of the process holds
    /// (the larger of its resident bytes and every charge counted, `used`'s own given back).
    /// Refused, typed and naming the platform, when the limit can't be read.
    pub fn room(&self, used: Use) -> Result<u64, Refusal> {
        self.room_and_limit(used).map(|(room, _)| room)
    }

    /// `bytes` more held by `used`, refused typed (past its room, [`Refusal::TooLarge`] naming the
    /// use and the room) when they would pass it.
    pub fn take(self: &Arc<Self>, used: Use, bytes: u64) -> Result<Charge, Refusal> {
        self.hold(used, bytes)?;
        Ok(Charge {
            budget: Arc::clone(self),
            used,
            bytes,
        })
    }

    fn hold_with_limit_until(
        &self,
        used: Use,
        bytes: u64,
        until: Option<Instant>,
    ) -> Result<(), (Refusal, Option<u64>)> {
        let reading = self.readings.read();
        let limit = reading
            .limit
            .ok_or_else(|| (unread("memory limit"), None))?;
        let room_for = |total: u64, own: u64| {
            let charged = total.max(own);
            let resident = reading.resident.unwrap_or(charged);
            let others = resident.max(charged).saturating_sub(own);
            limit.saturating_sub(others)
        };
        if bytes > limit {
            let own = self.held(used);
            let room = room_for(self.total_held.load(Ordering::Acquire), own);
            return Err((
                Refusal::TooLarge {
                    what: format!("memory for {}", used.name()),
                    size: usize::try_from(own.saturating_add(bytes)).unwrap_or(usize::MAX),
                    limit: usize::try_from(room).unwrap_or(usize::MAX),
                },
                Some(limit),
            ));
        }
        // Check deadline *after* `readings.read()` and *before* committing any reservation so a
        // slow system reading cannot grant admission past `until`.
        if until.is_some_and(|deadline| Instant::now() >= deadline) {
            let own = self.held(used);
            let room = room_for(self.total_held.load(Ordering::Acquire), own);
            return Err((
                Refusal::Full {
                    limit: usize::try_from(room).unwrap_or(usize::MAX),
                },
                Some(limit),
            ));
        }
        if bytes == 0 {
            return Ok(());
        }
        // Reserve atomically against the process-wide `total_held` counter first so concurrent
        // reservations across distinct `Use` categories cannot collectively exceed `limit`.
        self.total_held
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |total| {
                let after = total.checked_add(bytes)?;
                let effective = reading.resident.unwrap_or(total).max(total);
                effective
                    .checked_add(bytes)
                    .filter(|used_after| *used_after <= limit)
                    .map(|_| after)
            })
            .map(|_| {
                self.slot(used).fetch_add(bytes, Ordering::AcqRel);
            })
            .map_err(|total| {
                let own = self.held(used);
                let room = room_for(total, own);
                (
                    Refusal::TooLarge {
                        what: format!("memory for {}", used.name()),
                        size: usize::try_from(own.saturating_add(bytes)).unwrap_or(usize::MAX),
                        limit: usize::try_from(room).unwrap_or(usize::MAX),
                    },
                    Some(limit),
                )
            })
    }

    /// `bytes` more held by `used`, as [`Budget::take`], for a holder that counts its own and
    /// gives them back with [`Budget::give`] (the engine's versioned storage, which takes a run of
    /// frames or a step of its versions' reservation at a time).
    pub fn hold(&self, used: Use, bytes: u64) -> Result<(), Refusal> {
        self.hold_with_limit_until(used, bytes, None)
            .map_err(|(refusal, _)| refusal)
    }

    /// `bytes` more held by `used`, as [`Budget::take`], waiting for room until `until`: woken by
    /// each charge given back anywhere in the budget ([`Budget::give`]), never by a timer, and
    /// `waits` told (under the budget's lock) each time it waits. Refused [`Refusal::Full`], with
    /// the room it last saw, once `until` has passed. Refused at once, never waited on: bytes past
    /// the whole limit, [`Refusal::TooLarge`] naming it (no charge given back makes room for them;
    /// lane perf-A19), and a refusal other than no room.
    pub fn take_until(
        self: &Arc<Self>,
        used: Use,
        bytes: u64,
        until: Instant,
        mut waits: impl FnMut(),
    ) -> Result<Charge, Refusal> {
        // Room found takes no lock, as Linux's `wait_event` looks before it joins the queue
        // (include/linux/wait.h); the look under the lock below is the one a give can't slip past.
        match self.hold_with_limit_until(used, bytes, Some(until)) {
            Ok(()) => {
                return Ok(Charge {
                    budget: Arc::clone(self),
                    used,
                    bytes,
                });
            }
            Err((_, Some(limit))) if bytes > limit => {
                return Err(Refusal::TooLarge {
                    what: format!("memory for {}", used.name()),
                    size: usize::try_from(bytes).unwrap_or(usize::MAX),
                    limit: usize::try_from(limit).unwrap_or(usize::MAX),
                });
            }
            Err((Refusal::TooLarge { .. }, _)) => {}
            Err((other, _)) => return Err(other),
        }
        let mut given = self.given.0.lock().unwrap_or_else(PoisonError::into_inner);
        self.waiting.fetch_add(1, Ordering::Relaxed);
        // Pairs with `give`'s sequentially consistent give-back and read of `waiting` (C++
        // [atomics.order], the single total order; Linux's include/linux/wait.h, `waitqueue_active`):
        // either this look for room sees bytes given back, or their giver sees this holder waiting.
        fence(Ordering::SeqCst);
        let held = 'look: loop {
            let seen = *given;
            let (room, whole_limit) = match self.hold_with_limit_until(used, bytes, Some(until)) {
                Ok(()) => break Ok(()),
                Err((Refusal::TooLarge { limit, .. }, whole_limit)) => (limit, whole_limit),
                Err((other, _)) => break Err(other),
            };
            if let Some(limit) = whole_limit.filter(|limit| bytes > *limit) {
                break Err(Refusal::TooLarge {
                    what: format!("memory for {}", used.name()),
                    size: usize::try_from(bytes).unwrap_or(usize::MAX),
                    limit: usize::try_from(limit).unwrap_or(usize::MAX),
                });
            }
            let refused = Refusal::Full { limit: room };
            let mut left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break Err(refused);
            }
            waits();
            while *given == seen {
                if left.is_zero() {
                    break 'look Err(refused);
                }
                given = self
                    .given
                    .1
                    .wait_timeout(given, left)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
                left = until.saturating_duration_since(Instant::now());
            }
        };
        self.waiting.fetch_sub(1, Ordering::Relaxed);
        drop(given);
        held.map(|()| Charge {
            budget: Arc::clone(self),
            used,
            bytes,
        })
    }

    /// The holders waiting for room now ([`Budget::take_until`]).
    #[must_use]
    pub fn waiting(&self) -> u64 {
        self.waiting.load(Ordering::Relaxed)
    }

    /// `bytes` held by `used` ([`Budget::hold`]) given back, and the holders waiting for room
    /// ([`Budget::take_until`]) woken to look again.
    pub fn give(&self, used: Use, bytes: u64) {
        let mut removed = 0u64;
        let _ = self
            .slot(used)
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |held| {
                removed = held.min(bytes);
                Some(held.saturating_sub(bytes))
            });
        if removed > 0 {
            let _ = self
                .total_held
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |total| {
                    Some(total.saturating_sub(removed))
                });
        }
        if self.waiting.load(Ordering::SeqCst) > 0 {
            let mut given = self.given.0.lock().unwrap_or_else(PoisonError::into_inner);
            *given = given.wrapping_add(1);
            drop(given);
            self.given.1.notify_all();
        }
    }

    /// The room for one image: [`Use::Images`]' room, in the `usize` an image's bytes are
    /// counted in (the seam owner's ruling (c): an image is bounded by the memory budget's room
    /// for one; SB.1).
    pub fn image_room(&self) -> Result<usize, Refusal> {
        self.room(Use::Images)
            .map(|room| usize::try_from(room).unwrap_or(usize::MAX))
    }
}

/// The refusal where `what` can't be read on this platform.
fn unread(what: &str) -> Refusal {
    Refusal::Unavailable(format!(
        "this process's {what} can't be read on {} ({}): no memory budget",
        std::env::consts::OS,
        std::env::consts::ARCH
    ))
}

/// The process's budget, over the system's readings; a test's own, where it set one on its thread.
#[must_use]
pub fn process() -> Arc<Budget> {
    #[cfg(test)]
    if let Some(budget) = tests::BUDGET.with(|budget| budget.borrow().clone()) {
        return budget;
    }
    static PROCESS: OnceLock<Arc<Budget>> = OnceLock::new();
    PROCESS
        .get_or_init(|| Budget::new(Box::new(System::new())))
        .clone()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::Mutex;

    thread_local! {
        /// A test's budget for what it opens on its thread, in place of the process's.
        pub(crate) static BUDGET: RefCell<Option<Arc<Budget>>> = const { RefCell::new(None) };
    }

    /// Readings a test feeds and changes as it goes.
    #[derive(Clone, Default)]
    pub(crate) struct Fed(pub(crate) Arc<Mutex<Option<Reading>>>);

    impl Fed {
        pub(crate) fn new(limit: Option<u64>, resident: Option<u64>) -> Self {
            Self(Arc::new(Mutex::new(Some(Reading { limit, resident }))))
        }

        pub(crate) fn set(&self, limit: Option<u64>, resident: Option<u64>) {
            if let Ok(mut reading) = self.0.lock() {
                *reading = Some(Reading { limit, resident });
            }
        }
    }

    impl Readings for Fed {
        fn read(&self) -> Reading {
            self.0.lock().ok().and_then(|r| *r).unwrap_or(Reading {
                limit: None,
                resident: None,
            })
        }
    }

    /// A budget over `fed`, set as this thread's process budget until the guard drops.
    pub(crate) fn budget(fed: &Fed) -> (Arc<Budget>, impl Drop) {
        struct Unset;
        impl Drop for Unset {
            fn drop(&mut self) {
                BUDGET.with(|budget| budget.borrow_mut().take());
            }
        }
        let budget = Budget::new(Box::new(fed.clone()));
        BUDGET.with(|slot| *slot.borrow_mut() = Some(Arc::clone(&budget)));
        (budget, Unset)
    }

    /// T-T1.12.M1: a use's streams share its room. Do: open one stream of chunks, then a second,
    /// then close the first. Expect: one stream has the whole room, two half each, and a closed
    /// stream gives its share back.
    #[test]
    fn t_t1_12_m1_streams_share_a_uses_room() {
        let fed = Fed::new(Some(1 << 20), Some(0));
        let budget = Budget::new(Box::new(fed));
        assert_eq!(budget.stream_room(Use::Chunks).unwrap(), 1 << 20);
        let first = budget.stream(Use::Chunks);
        assert_eq!(budget.stream_room(Use::Chunks).unwrap(), 1 << 20);
        let second = budget.stream(Use::Chunks);
        assert_eq!(budget.streams(Use::Chunks), 2);
        assert_eq!(budget.stream_room(Use::Chunks).unwrap(), 1 << 19);
        assert_eq!(budget.stream_room(Use::Images).unwrap(), 1 << 20);
        drop(first);
        assert_eq!(budget.stream_room(Use::Chunks).unwrap(), 1 << 20);
        drop(second);
        assert_eq!(budget.streams(Use::Chunks), 0);
    }

    /// Do: a budget fed a 1 GiB limit and 100 MiB resident; take queue bytes, then image bytes;
    /// drop the queue's charge. Expect: each use's room is the limit less what the rest holds (the
    /// resident bytes until the charges pass them), a take past its room refused typed, and a
    /// charge given back on drop (MB.1, AC-MB.1.2).
    #[test]
    fn mb1_a_uses_room_is_what_the_rest_of_the_process_leaves() {
        let fed = Fed::new(Some(1 << 30), Some(100 << 20));
        let budget = Budget::new(Box::new(fed.clone()));
        assert_eq!(budget.room(Use::Images).unwrap(), (1 << 30) - (100 << 20));
        let queued = budget.take(Use::Queues, 600 << 20).unwrap();
        // The charges (600 MiB) now pass the resident bytes: the image has what they leave.
        assert_eq!(budget.room(Use::Images).unwrap(), (1 << 30) - (600 << 20));
        // The queue's own room gives its charge back.
        assert_eq!(budget.room(Use::Queues).unwrap(), 1 << 30);
        let refused = budget.take(Use::Images, 500 << 20);
        assert!(matches!(refused, Err(Refusal::TooLarge { what, limit, .. })
            if what.contains("image") && limit == (1 << 30) - (600 << 20)));
        let image = budget.take(Use::Images, 400 << 20).unwrap();
        assert_eq!(image.bytes(), 400 << 20);
        assert_eq!(budget.held(Use::Images), 400 << 20);
        drop(queued);
        assert_eq!(budget.held(Use::Queues), 0);
        // Resident bytes measured past the charges count as the rest's use.
        fed.set(Some(1 << 30), Some(900 << 20));
        assert_eq!(budget.room(Use::Images).unwrap(), (1 << 30) - (500 << 20));
        assert_eq!(budget.room(Use::Caches).unwrap(), (1 << 30) - (900 << 20));
        drop(image);
        assert_eq!(budget.held(Use::Images), 0);
    }

    /// Do: a budget whose limit can't be read, then is read, then is lowered as the process runs.
    /// Expect: a typed refusal naming the platform while unread, no fallback; then the room
    /// follows the limit as it moves (MB.1, AC-MB.1.1).
    #[test]
    fn mb1_no_limit_read_is_a_typed_refusal_and_the_limit_is_read_again() {
        let fed = Fed::new(None, Some(10 << 20));
        let budget = Budget::new(Box::new(fed.clone()));
        let refused = budget.room(Use::Queues);
        assert!(matches!(&refused, Err(Refusal::Unavailable(why))
            if why.contains(std::env::consts::OS) && why.contains("memory limit")));
        assert!(budget.take(Use::Queues, 1).is_err());
        fed.set(Some(64 << 20), Some(10 << 20));
        assert_eq!(budget.room(Use::Queues).unwrap(), 54 << 20);
        fed.set(Some(32 << 20), Some(10 << 20));
        assert_eq!(budget.room(Use::Queues).unwrap(), 22 << 20);
        // A limit below what is resident leaves no room, never a negative one.
        fed.set(Some(8 << 20), Some(10 << 20));
        assert_eq!(budget.room(Use::Images).unwrap(), 0);
        assert!(budget.take(Use::Images, 1).is_err());
        assert!(budget.take(Use::Images, 0).is_ok());
    }

    /// Do: a budget whose resident bytes can't be read. Expect: the charges counted stand for the
    /// rest's use (MB.1).
    #[test]
    fn mb1_unread_resident_bytes_fall_to_the_charges_counted() {
        let fed = Fed::new(Some(100), None);
        let budget = Budget::new(Box::new(fed));
        let held = budget.take(Use::Caches, 30).unwrap();
        assert_eq!(budget.room(Use::Images).unwrap(), 70);
        assert_eq!(budget.room(Use::Caches).unwrap(), 100);
        drop(held);
        assert_eq!(budget.room(Use::Images).unwrap(), 100);
    }

    /// Lane perf-A19 (AUDIT 2026-11-23 §1, MB.1): a charge past the whole limit is refused at once,
    /// never waited on. Do: on a budget with a 1 MiB limit and nothing held, take a byte past the
    /// limit with a deadline an hour off; then, with the whole limit held, take a byte with a
    /// deadline already passed; then, the limit given back, take the whole limit. Expect: the first
    /// refused `TooLarge` naming its size and the limit, no wait told; the second refused `Full`
    /// with no room, no wait told; the third taken; nothing held once it drops.
    #[test]
    fn a19_a_charge_past_the_whole_limit_is_refused_at_once_never_waited_on() {
        let fed = Fed::new(Some(1 << 20), Some(0));
        let budget = Budget::new(Box::new(fed));
        let far = Instant::now() + std::time::Duration::from_secs(3600);
        let mut waits = 0u32;
        let refused = budget.take_until(Use::Queues, (1 << 20) + 1, far, || waits += 1);
        assert!(
            matches!(&refused, Err(Refusal::TooLarge { what, size, limit })
                if what.contains("queues") && *size == (1 << 20) + 1 && *limit == 1 << 20),
            "{:?}",
            refused.err()
        );
        assert_eq!(waits, 0);
        let image = budget.take(Use::Images, 1 << 20).unwrap();
        let refused = budget.take_until(Use::Queues, 1, Instant::now(), || waits += 1);
        assert_eq!(refused.err(), Some(Refusal::Full { limit: 0 }));
        assert_eq!(waits, 0);
        drop(image);
        let whole = budget
            .take_until(Use::Queues, 1 << 20, far, || waits += 1)
            .unwrap();
        assert_eq!((whole.bytes(), waits), (1 << 20, 0));
        drop(whole);
        assert_eq!(budget.held(Use::Queues), 0);
    }

    /// Do: read this process's limit and resident bytes where it runs (Linux, macOS, Windows).
    /// Expect: both are read, the resident bytes within the limit (MB.1, AC-MB.1.1: learned where
    /// it runs, on all three).
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    #[test]
    fn mb1_this_process_reads_its_limit_and_resident_bytes() {
        let limit = limit();
        let resident = resident();
        assert!(limit.is_some_and(|l| l > 0));
        assert!(resident.is_some_and(|r| r > 0));
        assert!(resident <= limit);
        assert!(process().room(Use::Images).is_ok());
    }

    /// AUDIT 2026-11-23 §1: the system's readings from files opened once are the readings taken
    /// afresh. Do: read the limit both ways, and the resident bytes from the open statm. Expect:
    /// the same limit (MemTotal through `sysinfo`), resident bytes read and within it, and the
    /// files still this process's (MB.1, AC-MB.1.1).
    #[cfg(target_os = "linux")]
    #[test]
    fn mb1_opened_files_read_what_a_fresh_read_does() {
        let opened = Opened::open().unwrap();
        assert!(opened.current());
        assert_eq!(opened.limit(), limit());
        let resident = opened.resident();
        assert!(resident.is_some_and(|r| r > 0));
        assert!(resident <= opened.limit());
        let system = System::new();
        let first = system.read();
        let again = system.read();
        assert_eq!(first.limit, limit());
        assert_eq!(again.limit, first.limit);
        assert!(again.resident.is_some());
    }

    /// AC-MB.1.1. A group whose files could not be opened must be retried even if membership
    /// stays the same. After the files become available its limit bounds admission again.
    #[cfg(target_os = "linux")]
    #[test]
    fn review_retry_incomplete_cgroup_files() {
        let dir = tempfile::tempdir().unwrap();
        let membership = dir.path().join("membership");
        std::fs::write(&membership, "0::/child\n").unwrap();
        let child = dir.path().join("child");
        std::fs::create_dir(&child).unwrap();
        let open = || Opened::open_groups(std::fs::File::open(&membership).unwrap(), dir.path());
        assert!(
            open().is_none(),
            "cached a group with neither limit nor watcher"
        );
        std::fs::write(child.join("memory.max"), "1024\n").unwrap();
        assert_eq!(open().unwrap().limit(), Some(1024));
    }

    /// AC-MB.1.1. A previously opened limit which can no longer be read invalidates the cached
    /// set, so discovery will retry the same group's current files.
    #[cfg(target_os = "linux")]
    #[test]
    fn review_unreadable_cgroup_limit_requires_rediscovery() {
        let dir = tempfile::tempdir().unwrap();
        let membership = dir.path().join("membership");
        std::fs::write(&membership, "0::/child\n").unwrap();
        let child = dir.path().join("child");
        std::fs::create_dir(&child).unwrap();
        // A file descriptor whose positional read returns an error.
        std::fs::create_dir(child.join("memory.max")).unwrap();
        let opened =
            Opened::open_groups(std::fs::File::open(membership).unwrap(), dir.path()).unwrap();
        assert_eq!(opened.limit(), None);
    }

    /// AC-MB.1.1. Do: cache this process's opened files with a limit file that then fails every
    /// read, and take the system's reading. Expect: the limit a fresh read gives, from a newly
    /// opened set that replaces the failing one, not the machine's limit from the rest.
    #[cfg(target_os = "linux")]
    #[test]
    fn review_a_cached_limit_that_stops_reading_is_rediscovered() {
        let mut cached = Opened::open().unwrap();
        let dir = tempfile::tempdir().unwrap();
        cached.limits = vec![std::fs::File::open(dir.path()).unwrap()];
        let cached = Arc::new(cached);
        let system = System::new();
        *system.0.inner.opened.write().unwrap() = Some(cached.clone());
        assert_eq!(system.read().limit, limit());
        assert!(!Arc::ptr_eq(
            system.0.inner.opened.read().unwrap().as_ref().unwrap(),
            &cached
        ));
    }

    /// Do: parse a cgroup v2 process's `/proc/self/cgroup`, a v1 process's, and a hybrid's. Expect:
    /// each memory group's path, none where there is none (AC-T1.13.4: reads at their latency).
    #[test]
    fn a_process_finds_its_memory_group_in_either_hierarchy() {
        let v2 = "0::/user.slice/user-1000.slice/app.slice/amphora.service\n";
        assert_eq!(
            v2_group(v2),
            Some("/user.slice/user-1000.slice/app.slice/amphora.service")
        );
        assert_eq!(v1_group(v2), None);
        let v1 = "12:pids:/a\n11:memory:/docker/abc\n10:cpu,cpuacct:/docker/abc\n";
        assert_eq!(v1_group(v1), Some("/docker/abc"));
        assert_eq!(v2_group(v1), None);
        let hybrid = "11:memory:/k8s/pod1\n0::/k8s/pod1\n";
        assert_eq!(v1_group(hybrid), Some("/k8s/pod1"));
        assert_eq!(v2_group(hybrid), Some("/k8s/pod1"));
        assert_eq!(v2_group("0::/\n"), Some("/"));
    }

    /// Do: read limits as the kernel writes them. Expect: bytes for a number, none for "max" and
    /// for v1's no-limit value, none for text that isn't one (AC-T1.13.4: reads at their latency).
    #[test]
    fn a_limit_is_a_number_of_bytes_or_none() {
        assert_eq!(group_limit("34359738368\n"), Some(34_359_738_368));
        assert_eq!(group_limit("max\n"), None);
        assert_eq!(group_limit("9223372036854771712\n"), None);
        assert_eq!(group_limit("garbage"), None);
        let meminfo = "MemTotal:       131072000 kB\nMemFree:        1000 kB\n";
        assert_eq!(mem_total(meminfo), Some(131_072_000 * 1024));
        assert_eq!(mem_total("MemFree: 1 kB\n"), None);
    }

    /// Do: list the groups above `/a/b/c`, `/a` and the root. Expect: deepest first, the root last,
    /// the root once (AC-T1.13.4: reads at their latency).
    #[test]
    fn a_groups_limit_is_bounded_by_every_group_above_it() {
        assert_eq!(ancestors("/a/b/c"), ["/a/b/c", "/a/b", "/a", "/"]);
        assert_eq!(ancestors("/a"), ["/a", "/"]);
        assert_eq!(ancestors("/"), ["/"]);
        assert_eq!(ancestors(""), ["/"]);
    }

    /// Do: ask this process's limit on Linux. Expect: some number of bytes, no more than the
    /// machine's memory (AC-T1.13.4: reads at their latency).
    #[cfg(target_os = "linux")]
    #[test]
    fn this_process_has_a_limit_no_larger_than_its_machine() {
        let machine = std::fs::read_to_string("/proc/meminfo")
            .ok()
            .as_deref()
            .and_then(mem_total);
        let got = limit();
        assert!(got.is_some());
        assert!(got <= machine);
    }

    /// P2 regression (AC-MB.1.1): when a memory limit is reduced while one thread is mid-sample,
    /// callers arriving after the reduction must never reuse the previous completed sample; they
    /// must coalesce onto a fresh reading started after they arrived and observe the reduced limit.
    #[test]
    fn p2_ac_mb1_1_single_flight_never_serves_obsolete_limit_after_reduction() {
        use std::sync::atomic::AtomicUsize;

        struct GateReadings {
            limit: AtomicU64,
            calls: AtomicUsize,
            pause_on_call: AtomicUsize,
            entered: (Mutex<bool>, Condvar),
            release: (Mutex<bool>, Condvar),
        }

        impl Readings for GateReadings {
            fn read(&self) -> Reading {
                let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
                let value = self.limit.load(Ordering::SeqCst);
                if n == self.pause_on_call.load(Ordering::SeqCst) {
                    {
                        let mut flag = self.entered.0.lock().unwrap();
                        *flag = true;
                        self.entered.1.notify_all();
                    }
                    let mut go = self.release.0.lock().unwrap();
                    while !*go {
                        go = self.release.1.wait(go).unwrap();
                    }
                }
                Reading {
                    limit: Some(value),
                    resident: Some(0),
                }
            }
        }

        let sf = Arc::new(SingleFlight::new(GateReadings {
            limit: AtomicU64::new(100 << 20),
            calls: AtomicUsize::new(0),
            pause_on_call: AtomicUsize::new(2),
            entered: (Mutex::new(false), Condvar::new()),
            release: (Mutex::new(false), Condvar::new()),
        }));

        // Populate a completed prior sample at 100 MiB (call #1).
        assert_eq!(sf.read().limit, Some(100 << 20));
        assert_eq!(sf.inner.calls.load(Ordering::SeqCst), 1);

        // Thread 1 begins call #2 (reads 100 MiB, then pauses mid-flight).
        let sf1 = Arc::clone(&sf);
        let t1 = std::thread::spawn(move || sf1.read());
        {
            let mut flag = sf.inner.entered.0.lock().unwrap();
            while !*flag {
                flag = sf.inner.entered.1.wait(flag).unwrap();
            }
        }

        // System limit drops to 10 MiB while call #2 is still in flight.
        sf.inner.limit.store(10 << 20, Ordering::SeqCst);

        // Spawn 4 callers arriving strictly AFTER the limit reduction.
        let followers: Vec<_> = (0..4)
            .map(|_| {
                let sf_f = Arc::clone(&sf);
                std::thread::spawn(move || sf_f.read())
            })
            .collect();

        // Wait deterministically until all 4 followers are parked on the next flight, then release
        // call #2 so all 4 coalesce onto one fresh reading started strictly after the reduction.
        while sf.waiters.load(Ordering::Acquire) < 4 {
            std::thread::yield_now();
        }
        {
            let mut go = sf.inner.release.0.lock().unwrap();
            *go = true;
            sf.inner.release.1.notify_all();
        }

        assert_eq!(t1.join().unwrap().limit, Some(100 << 20));
        for f in followers {
            assert_eq!(
                f.join().unwrap().limit,
                Some(10 << 20),
                "AC-MB.1.1: ask after limit reduction must never observe obsolete 100 MiB sample"
            );
        }
        // Initial read + t1 + exactly 1 coalesced fresh read shared by all 4 followers.
        assert_eq!(sf.inner.calls.load(Ordering::SeqCst), 3);
    }

    /// Stress test (AC-MB.1.1): 32 concurrent callers hammering `SingleFlight::read()` across
    /// thousands of overlapping generations, verifying strict linearizability (every returned
    /// sequence >= sequence observed before calling `read()`), clean recovery after an inner
    /// panic, and return to `FAST_IDLE` with zero stranded waiters.
    #[test]
    fn single_flight_concurrent_waves_and_panic_recovery() {
        use std::sync::atomic::AtomicUsize;

        struct MonotonicReadings {
            seq: AtomicU64,
            panic_on_seq: AtomicU64,
            calls: AtomicUsize,
        }

        impl Readings for MonotonicReadings {
            fn read(&self) -> Reading {
                self.calls.fetch_add(1, Ordering::Relaxed);
                let v = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
                if v == self.panic_on_seq.load(Ordering::Relaxed) {
                    panic!("injected readings panic at seq {v}");
                }
                // Brief work so threads overlap across fast->slow escalation and parity flips.
                std::hint::black_box(v);
                if v.is_multiple_of(7) {
                    std::thread::yield_now();
                }
                Reading {
                    limit: Some(v),
                    resident: Some(0),
                }
            }
        }

        let sf = Arc::new(SingleFlight::new(MonotonicReadings {
            seq: AtomicU64::new(0),
            panic_on_seq: AtomicU64::new(0),
            calls: AtomicUsize::new(0),
        }));

        // Wave 1: 32 threads x 400 reads each (12,800 reads across fast + slow + parity shifts).
        std::thread::scope(|s| {
            for _ in 0..32 {
                let sf = Arc::clone(&sf);
                s.spawn(move || {
                    for _ in 0..400 {
                        let before = sf.inner.seq.load(Ordering::SeqCst);
                        let got = sf.read().limit.unwrap();
                        assert!(
                            got > before,
                            "AC-MB.1.1 violation: read started after seq {before} returned stale {got}"
                        );
                    }
                });
            }
        });
        assert!(sf.is_idle());

        // Wave 2: inject a panic mid-flight while 16 threads are contending; surviving threads
        // must never deadlock and must observe strictly fresh readings.
        let panic_target = sf.inner.seq.load(Ordering::SeqCst) + 10;
        sf.inner.panic_on_seq.store(panic_target, Ordering::SeqCst);
        let mut panicked = 0usize;
        std::thread::scope(|s| {
            let handles: Vec<_> = (0..16)
                .map(|_| {
                    let sf = Arc::clone(&sf);
                    s.spawn(move || {
                        for _ in 0..50 {
                            let before = sf.inner.seq.load(Ordering::SeqCst);
                            let got = sf.read().limit.unwrap();
                            assert!(got > before);
                        }
                    })
                })
                .collect();
            for h in handles {
                if h.join().is_err() {
                    panicked += 1;
                }
            }
        });
        assert!(panicked <= 1, "at most the one panicking leader fails");
        assert!(sf.is_idle());
    }

    /// Review P1 regression: under a controlled panic-and-retry schedule across consecutive waves,
    /// a panicking leader must wake every follower in its wave and hand off cleanly so no caller
    /// is ever stranded asleep after all other threads finish.
    #[test]
    fn review_p1_single_flight_panic_recovery_never_strands_follower() {
        struct ScriptedSampler {
            call: AtomicUsize,
            entered: (Mutex<usize>, Condvar),
            release_call: (Mutex<usize>, Condvar),
        }

        impl ScriptedSampler {
            fn wait_entered(&self, target: usize) {
                let mut n = self.entered.0.lock().unwrap();
                while *n < target {
                    n = self.entered.1.wait(n).unwrap();
                }
            }

            fn release_up_to(&self, target: usize) {
                let mut g = self.release_call.0.lock().unwrap();
                *g = (*g).max(target);
                self.release_call.1.notify_all();
            }
        }

        impl Readings for ScriptedSampler {
            fn read(&self) -> Reading {
                let n = self.call.fetch_add(1, Ordering::SeqCst) + 1;
                {
                    let mut entered = self.entered.0.lock().unwrap();
                    *entered = (*entered).max(n);
                    self.entered.1.notify_all();
                }
                {
                    let mut released = self.release_call.0.lock().unwrap();
                    while *released < n {
                        released = self.release_call.1.wait(released).unwrap();
                    }
                }
                if n == 2 {
                    panic!("injected panic on wave 2");
                }
                Reading {
                    limit: Some(n as u64 * 100),
                    resident: Some(0),
                }
            }
        }

        let sf = Arc::new(SingleFlight::new(ScriptedSampler {
            call: AtomicUsize::new(0),
            entered: (Mutex::new(0), Condvar::new()),
            release_call: (Mutex::new(0), Condvar::new()),
        }));

        // Flight 1 (fast path): pauses inside sampler.
        let sf1 = Arc::clone(&sf);
        let t1 = std::thread::spawn(move || sf1.read());
        sf.inner.wait_entered(1);

        // Wave 2: leader (t2) + follower (t3) both park while flight 1 is active.
        let sf2 = Arc::clone(&sf);
        let t2 = std::thread::spawn(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sf2.read()))
        });
        let sf3 = Arc::clone(&sf);
        let t3 = std::thread::spawn(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sf3.read()))
        });
        while sf.waiters.load(Ordering::Acquire) < 2 {
            std::thread::yield_now();
        }

        // Release flight 1 -> hands off to wave 2 leader, which enters call #2 and pauses.
        sf.inner.release_up_to(1);
        assert_eq!(t1.join().unwrap().limit, Some(100));
        sf.inner.wait_entered(2);

        // While wave 2 is active, wave 3 leader (t4) and follower (t5) arrive and park.
        let sf4 = Arc::clone(&sf);
        let t4 = std::thread::spawn(move || sf4.read());
        let sf5 = Arc::clone(&sf);
        let t5 = std::thread::spawn(move || sf5.read());
        while sf.waiters.load(Ordering::Acquire) < 3 {
            std::thread::yield_now();
        }

        // Release call #2 (panics!) and allow all subsequent retries to complete.
        // No additional external caller arrives — every surviving thread must finish on its own.
        sf.inner.release_up_to(10);

        let r2 = t2.join().unwrap();
        let r3 = t3.join().unwrap();
        // Exactly one of (t2, t3) was the leader that panicked; the other retried and succeeded.
        assert_eq!(
            usize::from(r2.is_err()) + usize::from(r3.is_err()),
            1,
            "exactly the panicking leader should fail; follower must retry"
        );
        let survivor = r2.or(r3).unwrap();
        assert!(survivor.limit.unwrap() >= 300);
        assert!(t4.join().unwrap().limit.unwrap() >= 300);
        assert!(t5.join().unwrap().limit.unwrap() >= 300);
        assert!(sf.is_idle());
    }

    /// Review P2 regression: continuous fast-path readers must never starve a contended caller
    /// waiting to acquire the flight gate (no mutex and no unbounded CAS escalation loop).
    #[test]
    fn review_p2_single_flight_fast_readers_cannot_starve_contended_caller() {
        use std::sync::atomic::AtomicBool;

        struct PauseFirst {
            calls: AtomicUsize,
            entered_first: (Mutex<bool>, Condvar),
            release_first: (Mutex<bool>, Condvar),
        }

        impl Readings for PauseFirst {
            fn read(&self) -> Reading {
                let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
                if n == 1 {
                    {
                        let mut f = self.entered_first.0.lock().unwrap();
                        *f = true;
                        self.entered_first.1.notify_all();
                    }
                    let mut g = self.release_first.0.lock().unwrap();
                    while !*g {
                        g = self.release_first.1.wait(g).unwrap();
                    }
                }
                Reading {
                    limit: Some(n as u64),
                    resident: Some(0),
                }
            }
        }

        let sf = Arc::new(SingleFlight::new(PauseFirst {
            calls: AtomicUsize::new(0),
            entered_first: (Mutex::new(false), Condvar::new()),
            release_first: (Mutex::new(false), Condvar::new()),
        }));

        // First reader holds the gate in-flight.
        let sf1 = Arc::clone(&sf);
        let t1 = std::thread::spawn(move || sf1.read());
        {
            let mut f = sf.inner.entered_first.0.lock().unwrap();
            while !*f {
                f = sf.inner.entered_first.1.wait(f).unwrap();
            }
        }

        // Contended caller enrolls as next wave leader (`MODE_QUEUED`).
        let sf_contended = Arc::clone(&sf);
        let t_contended = std::thread::spawn(move || sf_contended.read());
        while sf.waiters.load(Ordering::Acquire) < 1 {
            std::thread::yield_now();
        }
        assert_eq!(state_mode(sf.state.load(Ordering::Acquire)), MODE_QUEUED);

        // Start a swarm of fast-path hammer threads right before releasing t1.
        let stop = Arc::new(AtomicBool::new(false));
        let hammers: Vec<_> = (0..4)
            .map(|_| {
                let sf_h = Arc::clone(&sf);
                let stop_h = Arc::clone(&stop);
                std::thread::spawn(move || {
                    while !stop_h.load(Ordering::Relaxed) {
                        let _ = sf_h.read();
                    }
                })
            })
            .collect();

        {
            let mut g = sf.inner.release_first.0.lock().unwrap();
            *g = true;
            sf.inner.release_first.1.notify_all();
        }

        assert_eq!(t1.join().unwrap().limit, Some(1));
        // Because `release_active()` transfers `GATE_ACTIVE` directly to the waiting leader
        // without exposing `GATE_IDLE`, the contended caller MUST execute immediately as call #2!
        assert_eq!(
            t_contended.join().unwrap().limit,
            Some(2),
            "contended caller must receive direct handoff as call #2, never starved by fast readers"
        );
        stop.store(true, Ordering::Relaxed);
        for h in hammers {
            h.join().unwrap();
        }
        assert!(sf.is_idle());
    }

    /// Review P1 regression: concurrent reservations across all five `Use` categories must never
    /// collectively exceed the process memory limit.
    #[test]
    fn review_p1_budget_concurrent_categories_never_exceed_limit() {
        let fed = Fed::new(Some(100), Some(0));
        let budget = Budget::new(Box::new(fed));
        let barrier = Arc::new(std::sync::Barrier::new(Use::ALL.len()));

        for round in 0..256 {
            let charges: Vec<Option<Charge>> = std::thread::scope(|s| {
                let handles: Vec<_> = Use::ALL
                    .into_iter()
                    .map(|used| {
                        let b = Arc::clone(&budget);
                        let bar = Arc::clone(&barrier);
                        s.spawn(move || {
                            bar.wait();
                            b.take(used, 80).ok()
                        })
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            let granted: u64 = charges.iter().flatten().map(Charge::bytes).sum();
            let reported_total: u64 = Use::ALL.iter().map(|u| budget.held(*u)).sum();
            assert!(
                granted <= 100 && reported_total <= 100,
                "round {round}: concurrent reservations overshot 100-byte limit: granted={granted}, reported={reported_total}"
            );
            assert_eq!(
                granted, 80,
                "round {round}: exactly one 80-byte take must win"
            );
            drop(charges);
            assert_eq!(budget.total_held.load(Ordering::Acquire), 0);
        }
    }

    /// Review P2 regression: `Budget::take_until` must not grant a charge if `readings.read()`
    /// takes longer than the caller's deadline.
    #[test]
    fn review_p2_budget_take_until_rejects_expired_deadline_after_slow_read() {
        struct SlowReadings {
            gate: (Mutex<bool>, Condvar),
        }
        impl Readings for SlowReadings {
            fn read(&self) -> Reading {
                let guard = self.gate.0.lock().unwrap();
                let _ = self
                    .gate
                    .1
                    .wait_timeout(guard, std::time::Duration::from_millis(25))
                    .unwrap();
                Reading {
                    limit: Some(1 << 20),
                    resident: Some(0),
                }
            }
        }

        let budget = Budget::new(Box::new(SlowReadings {
            gate: (Mutex::new(false), Condvar::new()),
        }));
        let deadline = Instant::now() + std::time::Duration::from_millis(2);
        let mut waits = 0usize;
        let res = budget.take_until(Use::Queues, 64, deadline, || waits += 1);
        assert!(
            matches!(res, Err(Refusal::Full { .. })),
            "expected Refusal::Full after deadline expired during reading, got {:?}",
            res.err()
        );
        assert_eq!(waits, 0);
        assert_eq!(budget.held(Use::Queues), 0);
        assert_eq!(budget.total_held.load(Ordering::Acquire), 0);
    }
}
