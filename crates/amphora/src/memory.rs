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
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::Waker;

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
    let text = text.trim();
    if text == "max" {
        return None;
    }
    text.parse::<u64>().ok().filter(|&n| n < V1_UNLIMITED)
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

/// The memory this process may use, in bytes: the smallest of what the platform says (the module's
/// head lists the readings). None where nothing can be read.
#[must_use]
pub fn limit() -> Option<u64> {
    let mut limits = platform_limits();
    limits.extend(process_limits());
    limits.into_iter().min()
}

/// Linux: the limits of every control group above this process (v2 and v1) and the machine's
/// memory (`/proc/meminfo`).
#[cfg(target_os = "linux")]
fn platform_limits() -> Vec<u64> {
    let read = |path: &str| std::fs::read_to_string(path).ok();
    let cgroup = read("/proc/self/cgroup").unwrap_or_default();
    let mut limits: Vec<u64> = Vec::new();
    if let Some(group) = v2_group(&cgroup) {
        for at in ancestors(group) {
            let dir = format!("/sys/fs/cgroup{}", if at == "/" { "" } else { at.as_str() });
            if let Some(n) = read(&format!("{dir}/memory.max"))
                .as_deref()
                .and_then(group_limit)
            {
                limits.push(n);
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
                limits.push(n);
            }
        }
    }
    if let Some(n) = read("/proc/meminfo").as_deref().and_then(mem_total) {
        limits.push(n);
    }
    limits
}

/// macOS and Windows: the machine's physical memory as the platform reports it (the engine makes
/// the call: this crate denies `unsafe`). A Windows job object's memory limit isn't read (it needs
/// `QueryInformationJobObject`): the machine's memory bounds such a process from above.
#[cfg(not(target_os = "linux"))]
fn platform_limits() -> Vec<u64> {
    amphora_engine::io::system_memory::physical_memory()
        .into_iter()
        .collect()
}

/// The process's own soft limits on what it may map (POSIX `getrlimit`: `RLIMIT_AS`,
/// `RLIMIT_DATA`, and `RLIMIT_RSS` where the system enforces it), each where one is set.
#[cfg(unix)]
fn process_limits() -> Vec<u64> {
    use rustix::process::{Resource, getrlimit};
    [Resource::As, Resource::Data, Resource::Rss]
        .into_iter()
        .filter_map(|resource| getrlimit(resource).current)
        .collect()
}

/// Windows has no per-process soft limit of this kind (its job objects aside, above).
#[cfg(windows)]
fn process_limits() -> Vec<u64> {
    Vec::new()
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
/// file a backup copies and a restore fetches (T-T1.12.M1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Use {
    Queues,
    Images,
    Caches,
    Chunks,
}

impl Use {
    const ALL: [Self; 4] = [Self::Queues, Self::Images, Self::Caches, Self::Chunks];

    fn name(self) -> &'static str {
        match self {
            Self::Queues => "the owners' queues",
            Self::Images => "an image",
            Self::Caches => "the caches",
            Self::Chunks => "a backup's chunk",
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

/// The system's readings, taken again at every ask: a host may change a limit while the process runs.
/// On Linux each file is opened once and read again in place at every ask ([`Opened`]): the same
/// readings, exact at each ask, without the opens, path walks and `/proc/meminfo` they cost.
struct System {
    #[cfg(target_os = "linux")]
    opened: std::sync::RwLock<Option<Arc<Opened>>>,
}

impl System {
    fn new() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            opened: std::sync::RwLock::new(None),
        }
    }
}

#[cfg(not(target_os = "linux"))]
impl Readings for System {
    fn read(&self) -> Reading {
        Reading {
            limit: limit(),
            resident: resident(),
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
        let n = file.read_at(&mut buf, u64::try_from(out.len()).ok()?).ok()?;
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
        let seen = whole(&cgroup)?;
        let text = String::from_utf8_lossy(&seen);
        let (mut limits, mut unlimited) = (Vec::new(), Vec::new());
        for (v2, group, root, file) in [
            (true, v2_group(&text), "/sys/fs/cgroup", "memory.max"),
            (false, v1_group(&text), "/sys/fs/cgroup/memory", "memory.limit_in_bytes"),
        ] {
            for at in group.map(ancestors).unwrap_or_default() {
                let dir = format!("{root}{}", if at == "/" { "" } else { at.as_str() });
                match std::fs::File::open(format!("{dir}/{file}")) {
                    Ok(open) => limits.push(open),
                    // v2's root has no limit file, and never grows one.
                    Err(_) if v2 && at != "/" => {
                        if let Ok(open) = std::fs::File::open(format!("{dir}/cgroup.controllers")) {
                            unlimited.push(open);
                        }
                    }
                    Err(_) => {}
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
        same && !self.unlimited.iter().any(|controllers| {
            read_at(controllers, &mut buf)
                .and_then(|n| buf.get(..n))
                .is_some_and(|text| text.split(u8::is_ascii_whitespace).any(|c| c == b"memory"))
        })
    }

    fn limit(&self) -> Option<u64> {
        let mut buf = [0u8; IN_PLACE];
        let groups = self.limits.iter().filter_map(|file| {
            let n = read_at(file, &mut buf)?;
            std::str::from_utf8(buf.get(..n)?).ok().and_then(group_limit)
        });
        let machine = rustix::system::sysinfo();
        let machine = u64::from(machine.mem_unit.max(1)).checked_mul(machine.totalram);
        groups
            .chain(machine)
            .chain(process_limits())
            .min()
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
}

#[cfg(target_os = "linux")]
impl Readings for System {
    fn read(&self) -> Reading {
        let held = self.opened.read().ok().and_then(|o| o.clone());
        let opened = match held {
            Some(opened) if opened.current() => Some(opened),
            _ => {
                let fresh = Opened::open().map(Arc::new);
                if let Ok(mut slot) = self.opened.write() {
                    slot.clone_from(&fresh);
                }
                fresh
            }
        };
        match opened {
            Some(opened) => Reading {
                limit: opened.limit(),
                resident: opened.resident().or_else(resident),
            },
            None => Reading {
                limit: limit(),
                resident: resident(),
            },
        }
    }
}

/// The process's memory budget (MB.1): its limit, read where it runs, shared out to each [`Use`]
/// as that use's measured bytes. Each use charges what it holds as it takes it ([`Budget::take`])
/// and gives it back as it lets it go; a use's room is the limit less what everything else holds,
/// the larger of the process's resident bytes and the charges counted, its own charge given back.
/// No share is picked: a use has what the others leave, as measured now.
pub struct Budget {
    readings: Box<dyn Readings>,
    held: [AtomicU64; 4],
    /// The streams of each use open now, each of which may hold a charge at once ([`Stream`]).
    streams: [AtomicU64; 4],
    /// Who waits for room, in the order they asked ([`Budget::when_room`]), and how many: a
    /// release or a reading with no one waiting takes no lock.
    waiting: Mutex<VecDeque<Waiter>>,
    waiters: AtomicU64,
    /// The next wait's name ([`Waiting`]).
    next_wait: AtomicU64,
    /// The limit the last reading found, one past its bytes (0: none read): a reading that finds
    /// it changed is room freed or taken that no release says, so it runs the waiters' turn.
    limit_seen: AtomicU64,
}

/// One wait for `used`'s room to reach `bytes` ([`Budget::when_room`]). It keeps its place and its
/// claim on the room from its ask until it withdraws, woken or not.
struct Waiter {
    id: u64,
    used: Use,
    bytes: u64,
    wake: Waker,
    woken: bool,
}

/// A wait for room ([`Budget::when_room`]), withdrawn when this drops: a waiter that goes away
/// leaves nothing queued, and the room it claimed passes to those after it.
pub struct Waiting {
    budget: Arc<Budget>,
    id: u64,
    used: Use,
}

impl Waiting {
    /// The wait asked again in its own place, for `bytes` now: a waiter woken that found the room
    /// gone, or its need grown, keeps its turn ahead of those who asked after it.
    pub fn again(&self, bytes: u64, wake: &Waker) {
        {
            let mut waiting = self.budget.waiting_now();
            match waiting.iter_mut().find(|waiter| waiter.id == self.id) {
                Some(waiter) => {
                    waiter.bytes = bytes;
                    waiter.wake.clone_from(wake);
                    waiter.woken = false;
                }
                None => waiting.push_back(Waiter {
                    id: self.id,
                    used: self.used,
                    bytes,
                    wake: wake.clone(),
                    woken: false,
                }),
            }
            self.budget.count(&waiting);
        }
        std::sync::atomic::fence(Ordering::SeqCst);
        self.budget.turn();
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        let withdrew = {
            let mut waiting = self.budget.waiting_now();
            let before = waiting.len();
            waiting.retain(|waiter| waiter.id != self.id);
            self.budget.count(&waiting);
            waiting.len() < before && !waiting.is_empty()
        };
        if withdrew {
            self.budget.turn();
        }
    }
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
                Refusal::TooLarge { limit, .. } => amphora_engine::LimboError::MemoryBudget {
                    what,
                    asked: bytes,
                    held: self.0.held(Use::Caches),
                    limit: u64::try_from(limit).unwrap_or(u64::MAX),
                },
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
            waiting: Mutex::default(),
            waiters: AtomicU64::new(0),
            next_wait: AtomicU64::new(0),
            limit_seen: AtomicU64::new(0),
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

    fn of(counters: &[AtomicU64; 4], used: Use) -> &AtomicU64 {
        let [queues, images, caches, chunks] = counters;
        match used {
            Use::Queues => queues,
            Use::Images => images,
            Use::Caches => caches,
            Use::Chunks => chunks,
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

    /// The bytes `used` may hold at most now: the limit less what the rest of the process holds
    /// (the larger of its resident bytes and every charge counted, `used`'s own given back).
    /// Refused, typed and naming the platform, when the limit can't be read.
    pub fn room(&self, used: Use) -> Result<u64, Refusal> {
        let reading = self.readings.read();
        let limit = seen_limit(reading);
        // A limit changed is the host's, never a release's (lane snapshots, coordinator decision
        // 1): the reading that finds it, at the next ask of any use's room, runs the waiters'
        // turn. One load when it stands, as it does but for a host's change.
        if self.limit_seen.load(Ordering::Relaxed) != limit
            && self.limit_seen.swap(limit, Ordering::Relaxed) != limit
            && self.waiters.load(Ordering::SeqCst) > 0
        {
            self.turn();
        }
        self.room_in(reading, used)
    }

    /// Every use's charges counted.
    fn charged(&self) -> u64 {
        Use::ALL
            .iter()
            .fold(0u64, |sum, each| sum.saturating_add(self.held(*each)))
    }

    /// [`Budget::room`] as of `reading`.
    fn room_in(&self, reading: Reading, used: Use) -> Result<u64, Refusal> {
        let limit = reading.limit.ok_or_else(|| unread("memory limit"))?;
        let charged = self.charged();
        let resident = reading.resident.unwrap_or(charged);
        let others = resident.max(charged).saturating_sub(self.held(used));
        Ok(limit.saturating_sub(others))
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

    /// `bytes` more held by `used`, as [`Budget::take`], for a holder that counts its own and
    /// gives them back with [`Budget::give`] (the engine's versioned storage, which takes a run of
    /// frames or a step of its versions' reservation at a time).
    pub fn hold(&self, used: Use, bytes: u64) -> Result<(), Refusal> {
        let room = self.room(used)?;
        self.slot(used)
            .try_update(Ordering::AcqRel, Ordering::Acquire, |held| {
                held.checked_add(bytes).filter(|after| *after <= room)
            })
            .map(|_| ())
            .map_err(|held| Refusal::TooLarge {
                what: format!("memory for {}", used.name()),
                size: usize::try_from(held.saturating_add(bytes)).unwrap_or(usize::MAX),
                limit: usize::try_from(room).unwrap_or(usize::MAX),
            })
    }

    /// `bytes` held by `used` ([`Budget::hold`]) given back: room freed, which wakes those waiting
    /// for it that now fit ([`Budget::when_room`]).
    pub fn give(&self, used: Use, bytes: u64) {
        // Sequentially consistent, against the fence in `when_room`: a waiter counted after this
        // release reads the room it freed, and one counted before is seen here. On x86 these cost
        // what acquire-release does (a locked update, a plain load), so a release with no one
        // waiting adds no fence.
        let _ = self
            .slot(used)
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |held| {
                Some(held.saturating_sub(bytes))
            });
        if self.waiters.load(Ordering::SeqCst) > 0 {
            self.turn();
        }
    }

    /// Wakes `wake` once `used`'s room ([`Budget::room`]) reaches `bytes` in its turn: at once if
    /// it does now, else at the release of a charge ([`Budget::give`]), the withdrawal of a wait
    /// before it, or the reading of a changed limit ([`Budget::room`]) that brings it there. The
    /// turn is head-of-queue reservation (lane snapshots, coordinator decision 2): in the order
    /// asked, each wait claims the free room its use lacks of its bytes, so a later ask, however
    /// small, has only the room past the needs of those before it, and none starves. A room that
    /// can't be read wakes it too, for the waiter to meet the refusal. Woken once; a waiter that
    /// finds no room asks again in its place ([`Waiting::again`]). Dropping the answer withdraws
    /// the wait.
    #[must_use]
    pub fn when_room(self: &Arc<Self>, used: Use, bytes: u64, wake: &Waker) -> Waiting {
        let id = self.next_wait.fetch_add(1, Ordering::Relaxed);
        {
            let mut waiting = self.waiting_now();
            waiting.push_back(Waiter {
                id,
                used,
                bytes,
                wake: wake.clone(),
                woken: false,
            });
            self.count(&waiting);
        }
        // Pairs with `give`'s sequentially consistent release and count.
        std::sync::atomic::fence(Ordering::SeqCst);
        self.turn();
        Waiting {
            budget: Arc::clone(self),
            id,
            used,
        }
    }

    /// The waiting queue, whatever a waker that panicked left it as.
    fn waiting_now(&self) -> std::sync::MutexGuard<'_, VecDeque<Waiter>> {
        self.waiting
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `waiting`'s length, for a release or a reading to see without the lock.
    fn count(&self, waiting: &VecDeque<Waiter>) {
        self.waiters.store(
            u64::try_from(waiting.len()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }

    /// The waiters' turn at a reading now, in the order they asked: each claims, out of the free
    /// room no one before it claimed, what its use's room lacks of its bytes, and is woken (once)
    /// when its use's room past the claims before it reaches them. One that doesn't fit claims
    /// every free byte left, so those after it have only their own use's held bytes.
    fn turn(&self) {
        let reading = self.readings.read();
        self.limit_seen
            .store(seen_limit(reading), Ordering::Relaxed);
        let woken: Vec<Waker> = {
            let mut waiting = self.waiting_now();
            let charged = self.charged();
            let free = reading.limit.map(|limit| {
                limit.saturating_sub(reading.resident.unwrap_or(charged).max(charged))
            });
            let mut claimed = 0u64;
            let mut woken = Vec::new();
            for waiter in waiting.iter_mut() {
                let fits = match (free, self.room_in(reading, waiter.used)) {
                    (Some(free), Ok(room)) => {
                        let need = waiter.bytes.saturating_sub(self.held(waiter.used));
                        let fits = room.saturating_sub(claimed) >= waiter.bytes;
                        claimed = claimed.saturating_add(need).min(free);
                        fits
                    }
                    _ => true,
                };
                if fits && !waiter.woken {
                    waiter.woken = true;
                    woken.push(waiter.wake.clone());
                }
            }
            woken
        };
        for wake in woken {
            wake.wake();
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

/// `reading`'s limit as [`Budget`]'s `limit_seen` keeps it: one past its bytes, 0 for none read.
fn seen_limit(reading: Reading) -> u64 {
    reading.limit.map_or(0, |limit| limit.saturating_add(1))
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

    /// A waker that counts its wakes.
    struct Count(AtomicU64);

    impl std::task::Wake for Count {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn counter() -> (Arc<Count>, Waker) {
        let count = Arc::new(Count(AtomicU64::new(0)));
        (Arc::clone(&count), Waker::from(count))
    }

    fn woken(count: &Count) -> u64 {
        count.0.load(Ordering::SeqCst)
    }

    const KIB: u64 = 1 << 10;

    /// A budget fed a 1 MiB limit and nothing resident, every byte of it held by the caches.
    fn full() -> (Arc<Budget>, Fed) {
        let fed = Fed::new(Some(1024 * KIB), Some(0));
        let budget = Budget::new(Box::new(fed.clone()));
        budget.hold(Use::Caches, 1024 * KIB).unwrap();
        (budget, fed)
    }

    /// Room freed (lane snapshots, R2's room-freed event): a release wakes the waits its room
    /// reaches, an event and never a poll. Do: on a full budget, two waits for 256 KiB of the
    /// queues' room; give back 128 KiB, 128 KiB more, then 256 KiB, then 512 KiB; both withdrawn;
    /// a wait with room to spare; one on a budget whose limit can't be read; one dropped before its
    /// room comes. Expect: no wake until a release brings the room; 256 KiB wakes the older alone,
    /// the next 256 KiB the younger; each woken once; a wait with room now, or no room reading,
    /// woken at once; a dropped wait never.
    #[test]
    fn snapshots_room_freed_wakes_the_waits_it_reaches() {
        let (budget, _fed) = full();
        let (older, older_wake) = counter();
        let (younger, younger_wake) = counter();
        let older_wait = budget.when_room(Use::Queues, 256 * KIB, &older_wake);
        let younger_wait = budget.when_room(Use::Queues, 256 * KIB, &younger_wake);
        assert_eq!((woken(&older), woken(&younger)), (0, 0), "no room, no wake");
        budget.give(Use::Caches, 128 * KIB);
        assert_eq!((woken(&older), woken(&younger)), (0, 0), "128 KiB fits neither");
        budget.give(Use::Caches, 128 * KIB);
        assert_eq!((woken(&older), woken(&younger)), (1, 0), "256 KiB wakes the older alone");
        budget.give(Use::Caches, 256 * KIB);
        assert_eq!((woken(&older), woken(&younger)), (1, 1), "the next 256 KiB the younger");
        budget.give(Use::Caches, 512 * KIB);
        assert_eq!((woken(&older), woken(&younger)), (1, 1), "each woken once");
        drop((older_wait, younger_wait));
        let (now, now_wake) = counter();
        let now_wait = budget.when_room(Use::Queues, 256 * KIB, &now_wake);
        assert_eq!(woken(&now), 1, "room there already wakes at once");
        let unread = Budget::new(Box::new(Fed::new(None, Some(0))));
        let (refused, refused_wake) = counter();
        let _refused = unread.when_room(Use::Images, 1, &refused_wake);
        assert_eq!(woken(&refused), 1, "an unread room wakes the wait to meet its refusal");
        drop(now_wait);
        budget.hold(Use::Caches, 1024 * KIB).unwrap();
        let (gone, gone_wake) = counter();
        drop(budget.when_room(Use::Queues, KIB, &gone_wake));
        budget.give(Use::Caches, 1024 * KIB);
        assert_eq!(woken(&gone), 0, "a wait dropped before its room is withdrawn");
    }

    /// Head-of-queue reservation, no starvation (lane snapshots, coordinator decision 2): the
    /// oldest wait gathers the room freed until it fits, and no later ask takes it, however small.
    /// Do: on a full budget, a wait for 256 KiB of an image's room, then one for 64 KiB of the
    /// queues'; give back 64 KiB three times, asking a second 64 KiB wait after the third; then
    /// 64 KiB more; the image's 256 KiB held and its wait withdrawn; then 128 KiB. Expect: no wake
    /// while the freed room is short of the head's 256 KiB, the later waits' 64 KiB though it
    /// would fit them; the head woken at 256 KiB, the later ones not; both later ones woken once
    /// 128 KiB more comes past the head's.
    #[test]
    fn snapshots_the_oldest_wait_is_never_passed_by_later_smaller_ones() {
        let (budget, _fed) = full();
        let (head, head_wake) = counter();
        let (later, later_wake) = counter();
        let head_wait = budget.when_room(Use::Images, 256 * KIB, &head_wake);
        let _later = budget.when_room(Use::Queues, 64 * KIB, &later_wake);
        for _ in 0..3 {
            budget.give(Use::Caches, 64 * KIB);
            assert_eq!((woken(&head), woken(&later)), (0, 0), "the freed room is the head's");
        }
        let (asked, asked_wake) = counter();
        let _asked = budget.when_room(Use::Queues, 64 * KIB, &asked_wake);
        assert_eq!(woken(&asked), 0, "a later ask doesn't take the head's room either");
        budget.give(Use::Caches, 64 * KIB);
        assert_eq!(
            (woken(&head), woken(&later), woken(&asked)),
            (1, 0, 0),
            "256 KiB freed wakes the head alone"
        );
        budget.hold(Use::Images, 256 * KIB).unwrap();
        drop(head_wait);
        assert_eq!((woken(&later), woken(&asked)), (0, 0), "the head took its room");
        budget.give(Use::Caches, 128 * KIB);
        assert_eq!((woken(&later), woken(&asked)), (1, 1), "the room past the head's goes on");
    }

    /// Head-of-queue reservation, no blocking past the head's need (lane snapshots, coordinator
    /// decision 2). Do: on a full budget, a wait for 256 KiB of an image's room and a later one
    /// for 64 KiB of the queues'; one release of 320 KiB. Then, on a second full budget, the same
    /// two waits, 128 KiB given back, and the head's wait withdrawn. Expect: one release that frees
    /// the head's need and the later's wakes both; a head withdrawn passes its room on at once,
    /// no release needed.
    #[test]
    fn snapshots_room_past_the_heads_need_goes_on_at_once() {
        let (budget, _fed) = full();
        let (head, head_wake) = counter();
        let (later, later_wake) = counter();
        let _head = budget.when_room(Use::Images, 256 * KIB, &head_wake);
        let _later = budget.when_room(Use::Queues, 64 * KIB, &later_wake);
        budget.give(Use::Caches, 320 * KIB);
        assert_eq!((woken(&head), woken(&later)), (1, 1), "one release wakes both");

        let (budget, _fed) = full();
        let (head, head_wake) = counter();
        let (later, later_wake) = counter();
        let head_wait = budget.when_room(Use::Images, 256 * KIB, &head_wake);
        let _later = budget.when_room(Use::Queues, 64 * KIB, &later_wake);
        budget.give(Use::Caches, 128 * KIB);
        assert_eq!((woken(&head), woken(&later)), (0, 0), "128 KiB is the head's");
        drop(head_wait);
        assert_eq!(woken(&later), 1, "a head withdrawn passes its room on");
    }

    /// A changed limit wakes (lane snapshots, coordinator decision 1): the host raising the limit
    /// frees room no release says, and the reading that finds it runs the waiters' turn. Do: on a
    /// full budget, a wait for 256 KiB of an image's room; the limit raised by 128 KiB and a use's
    /// room asked; then raised by 256 KiB and a charge taken. Expect: the first reading finds too
    /// little, no wake; the charge's reading of the higher limit wakes it, no release given.
    #[test]
    fn snapshots_a_changed_limit_wakes_the_waits_it_makes_room_for() {
        let (budget, fed) = full();
        let (waits, wake) = counter();
        let _wait = budget.when_room(Use::Images, 256 * KIB, &wake);
        fed.set(Some(1152 * KIB), Some(0));
        let _ = budget.room(Use::Caches).unwrap();
        assert_eq!(woken(&waits), 0, "128 KiB more falls short");
        fed.set(Some(1280 * KIB), Some(0));
        budget.hold(Use::Queues, 0).unwrap();
        assert_eq!(woken(&waits), 1, "the reading of 256 KiB more wakes it");
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
