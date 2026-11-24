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
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering, fence};
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

/// One slot of the double-buffered single-flight reading: holds the last completed kernel reading
/// in atomics so concurrent callers arriving while another thread is already inside kernel syscalls
/// read it lock-free without piling duplicate syscalls onto the OS, while every sequential call
/// still reads the kernel afresh (MB.1; no time constant or TTL).
struct ReadingSlot {
    limit: AtomicU64,
    resident: AtomicU64,
    /// Bit 0: `limit` is `Some`; bit 1: `resident` is `Some`.
    flags: AtomicU8,
}

impl ReadingSlot {
    fn new() -> Self {
        Self {
            limit: AtomicU64::new(0),
            resident: AtomicU64::new(0),
            flags: AtomicU8::new(0),
        }
    }

    fn store(&self, reading: Reading) {
        let mut flags = 0u8;
        if let Some(limit) = reading.limit {
            self.limit.store(limit, Ordering::Relaxed);
            flags |= 1;
        }
        if let Some(resident) = reading.resident {
            self.resident.store(resident, Ordering::Relaxed);
            flags |= 2;
        }
        self.flags.store(flags, Ordering::Relaxed);
    }

    fn load(&self) -> Reading {
        let flags = self.flags.load(Ordering::Relaxed);
        let limit = (flags & 1 != 0).then(|| self.limit.load(Ordering::Relaxed));
        let resident = (flags & 2 != 0).then(|| self.resident.load(Ordering::Relaxed));
        Reading { limit, resident }
    }
}

/// The system's readings, taken again at every ask: a host may change a limit while the process runs.
/// On Linux each file is opened once and read again in place at every ask ([`Opened`]): the same
/// readings, exact at each ask, without the opens, path walks and `/proc/meminfo` they cost.
/// Under concurrency, overlapping asks coalesce via a lock-free double-buffered single-flight so
/// only one thread executes the kernel syscalls at a time; sequential asks always query the kernel.
struct System {
    #[cfg(target_os = "linux")]
    opened: std::sync::RwLock<Option<Arc<Opened>>>,
    /// Even `2 * gen`: idle, last completed reading is in `slots[(gen - 1) & 1]` when `gen > 0`.
    /// Odd `2 * gen + 1`: one thread is querying the kernel and will write `slots[gen & 1]`.
    flight: AtomicU64,
    slots: [ReadingSlot; 2],
}

impl System {
    fn new() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            opened: std::sync::RwLock::new(None),
            flight: AtomicU64::new(0),
            slots: [ReadingSlot::new(), ReadingSlot::new()],
        }
    }

    fn slot(&self, index: u64) -> &ReadingSlot {
        let [even, odd] = &self.slots;
        if index & 1 == 0 { even } else { odd }
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

impl Readings for System {
    fn read(&self) -> Reading {
        loop {
            let state = self.flight.load(Ordering::Acquire);
            if state & 1 == 1 {
                let generation = state >> 1;
                if let Some(prev) = generation.checked_sub(1) {
                    let reading = self.slot(prev).load();
                    // Pairs with the writer's Release store and AcqRel CAS: `prev` is untouched
                    // until `flight` advances to `state + 2`.
                    fence(Ordering::Acquire);
                    let after = self.flight.load(Ordering::Relaxed);
                    if after == state || after == state.wrapping_add(1) {
                        return reading;
                    }
                    continue;
                }
                return self.read_fresh();
            }
            let in_flight = state.wrapping_add(1);
            if self
                .flight
                .compare_exchange(state, in_flight, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                struct Guard<'a> {
                    flight: &'a AtomicU64,
                    restore: u64,
                    done: bool,
                }
                impl Drop for Guard<'_> {
                    fn drop(&mut self) {
                        if !self.done {
                            self.flight.store(self.restore, Ordering::Release);
                        }
                    }
                }
                let mut guard = Guard {
                    flight: &self.flight,
                    restore: state,
                    done: false,
                };
                let reading = self.read_fresh();
                self.slot(state >> 1).store(reading);
                guard.done = true;
                self.flight.store(state.wrapping_add(2), Ordering::Release);
                return reading;
            }
        }
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
        let charged = Use::ALL
            .iter()
            .fold(0u64, |sum, each| sum.saturating_add(self.held(*each)));
        let resident = reading.resident.unwrap_or(charged);
        let others = resident.max(charged).saturating_sub(self.held(used));
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

    fn hold_with_limit(&self, used: Use, bytes: u64) -> Result<(), (Refusal, Option<u64>)> {
        let (room, whole_limit) = self
            .room_and_limit(used)
            .map_err(|refusal| (refusal, None))?;
        self.slot(used)
            .try_update(Ordering::AcqRel, Ordering::Acquire, |held| {
                held.checked_add(bytes).filter(|after| *after <= room)
            })
            .map(|_| ())
            .map_err(|held| {
                (
                    Refusal::TooLarge {
                        what: format!("memory for {}", used.name()),
                        size: usize::try_from(held.saturating_add(bytes)).unwrap_or(usize::MAX),
                        limit: usize::try_from(room).unwrap_or(usize::MAX),
                    },
                    Some(whole_limit),
                )
            })
    }

    /// `bytes` more held by `used`, as [`Budget::take`], for a holder that counts its own and
    /// gives them back with [`Budget::give`] (the engine's versioned storage, which takes a run of
    /// frames or a step of its versions' reservation at a time).
    pub fn hold(&self, used: Use, bytes: u64) -> Result<(), Refusal> {
        self.hold_with_limit(used, bytes)
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
        match self.hold_with_limit(used, bytes) {
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
            let (room, whole_limit) = match self.hold_with_limit(used, bytes) {
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
        let _ = self
            .slot(used)
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |held| {
                Some(held.saturating_sub(bytes))
            });
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
        *system.opened.write().unwrap() = Some(cached.clone());
        assert_eq!(system.read().limit, limit());
        assert!(!Arc::ptr_eq(
            system.opened.read().unwrap().as_ref().unwrap(),
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
}
