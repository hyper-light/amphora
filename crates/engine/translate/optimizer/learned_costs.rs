//! Amphora (QT.5; `docs/design/executor.md`, decision 8): the cost weights of a seek, a hash
//! table's build and probe, and a sort, learned where the engine runs instead of picked. Each is a
//! rate, nanoseconds a unit of its work, kept per database: calibrated once a process from the
//! work itself (one page of keys searched, hashed, probed and sorted), then moved by the times the
//! database's operators take as its queries run. The planner reads each weight as a multiple of
//! the time a row's visit takes, so the four sit on the scale of the costs around them
//! (`CostModelParams::with_learned`). The bench harness `bench/join_methods` checks what they choose;
//! it is not where they come from.

use std::hint::black_box;
use std::sync::OnceLock;
use std::time::Instant;

use parking_lot::Mutex;
use rustc_hash::FxHashMap;

/// The kinds of work the planner weighs, each timed in its own unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Work {
    /// A row visited: read and tested once. The unit the others are weighed in.
    Row,
    /// One search of an ordered index for a key.
    Seek,
    /// One row added to a hash table.
    HashBuild,
    /// One lookup of a hash table.
    HashProbe,
    /// Sorting, in rows times log2 rows, as the planner counts a sort
    /// (`cost::estimate_sort_cpu_cost`).
    Sort,
    /// One page read from the database's file, as the pager times it from the read's submission
    /// to its completion (`sqlite3_ondisk::begin_read_page`). The planner's unit of cost.
    PageRead,
}

/// Units of work and the nanoseconds they took.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Span {
    units: f64,
    nanos: f64,
}

/// One kind's running rate: the work of the epoch under way and of the one before it. An epoch is
/// as much work as calibration timed, so the rate follows the latest one to two calibrations' worth
/// of work and leaves older times behind as conditions move.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Rate {
    epoch: f64,
    previous: Span,
    current: Span,
}

impl Rate {
    /// The rate calibration measured: `units` of work in `nanos`, also the epoch's length.
    fn calibrated(units: f64, nanos: f64) -> Self {
        Self {
            epoch: units,
            previous: Span { units, nanos },
            current: Span::default(),
        }
    }

    /// Adds an observation. One with no work or a time that is not a finite, non-negative number
    /// says nothing about the rate, and is left out.
    fn observe(&mut self, units: f64, nanos: f64) {
        if !(units.is_finite() && units > 0.0 && nanos.is_finite() && nanos >= 0.0) {
            return;
        }
        self.current.units += units;
        self.current.nanos += nanos;
        if self.current.units >= self.epoch {
            self.previous = self.current;
            self.current = Span::default();
        }
    }

    /// Nanoseconds a unit, None before any time was seen.
    fn nanos_per_unit(&self) -> Option<f64> {
        let units = self.previous.units + self.current.units;
        let nanos = self.previous.nanos + self.current.nanos;
        (units > 0.0 && nanos > 0.0).then_some(nanos / units)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Rates {
    row: Rate,
    seek: Rate,
    hash_build: Rate,
    hash_probe: Rate,
    sort: Rate,
    page: Rate,
}

impl Rates {
    fn rate_mut(&mut self, work: Work) -> &mut Rate {
        match work {
            Work::Row => &mut self.row,
            Work::Seek => &mut self.seek,
            Work::HashBuild => &mut self.hash_build,
            Work::HashProbe => &mut self.hash_probe,
            Work::Sort => &mut self.sort,
            Work::PageRead => &mut self.page,
        }
    }

    fn weights(&self) -> Option<LearnedWeights> {
        let row = self.row.nanos_per_unit()?;
        Some(LearnedWeights {
            row_in_pages: self.page.nanos_per_unit().map(|page| row / page),
            seek: self.seek.nanos_per_unit()? / row,
            hash_build: self.hash_build.nanos_per_unit()? / row,
            hash_probe: self.hash_probe.nanos_per_unit()? / row,
            sort: self.sort.nanos_per_unit()? / row,
        })
    }
}

/// The four weights, each in rows' visits: what one unit of the work takes against one row read
/// and tested; and the row's visit itself in page reads, the planner's unit, once the database has
/// read a page from its file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LearnedWeights {
    /// A row's visit against a page read; None until the database has timed one (a database
    /// whose pages all came from memory so far has no page read to weigh it against).
    pub(crate) row_in_pages: Option<f64>,
    pub(crate) seek: f64,
    pub(crate) hash_build: f64,
    pub(crate) hash_probe: f64,
    pub(crate) sort: f64,
}

/// A database's running cost estimator, shared by its connections.
#[derive(Debug)]
pub(crate) struct LearnedCosts {
    rates: Mutex<Rates>,
}

impl LearnedCosts {
    /// An estimator that starts from this process's calibration.
    pub(crate) fn new() -> Self {
        Self::from_rates(*process_calibration())
    }

    fn from_rates(rates: Rates) -> Self {
        Self {
            rates: Mutex::new(rates),
        }
    }

    /// Records that `units` of `work` took `nanos`, as an operator timed it.
    pub(crate) fn observe(&self, work: Work, units: f64, nanos: f64) {
        self.rates.lock().rate_mut(work).observe(units, nanos);
    }

    /// The weights as they stand; None only if the clock never moved while calibrating.
    pub(crate) fn weights(&self) -> Option<LearnedWeights> {
        self.rates.lock().weights()
    }

    /// Anchors the page read's rate to one read timed at open (`time_page_read`), before the
    /// database has read a page of its own. It counts as one read beside the database's own, and
    /// the first epoch of those replaces it.
    pub(crate) fn anchor_page_read(&self, nanos: f64) {
        let mut rates = self.rates.lock();
        if !(nanos.is_finite() && nanos > 0.0) {
            return;
        }
        rates.page.previous = Span { units: 1.0, nanos };
    }
}

/// The rates measured once a process, on the hardware it runs on.
fn process_calibration() -> &'static Rates {
    static CALIBRATION: OnceLock<Rates> = OnceLock::new();
    CALIBRATION.get_or_init(|| {
        let start = Instant::now();
        calibrate(&mut || start.elapsed().as_nanos() as f64)
    })
}

/// SplitMix64's output function (Steele, Lea and Flood, "Fast splittable pseudorandom number
/// generators", OOPSLA 2014; its published constants): keys in no order, the same every run.
pub(crate) fn mix(i: u64) -> u64 {
    let mut z = i.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Times `work` `reps` times over, doubling `reps` up to `most_reps` until the time reaches
/// `target`: (repetitions, nanoseconds).
fn measure(
    clock: &mut dyn FnMut() -> f64,
    target: f64,
    most_reps: usize,
    work: &mut dyn FnMut(),
) -> (f64, f64) {
    let mut reps: usize = 1;
    loop {
        let start = clock();
        for _ in 0..reps {
            work();
        }
        let nanos = clock() - start;
        match reps.checked_mul(2) {
            Some(next) if nanos < target && next <= most_reps => reps = next,
            _ => return (reps as f64, nanos),
        }
    }
}

/// Times each kind of work over one page of 8-byte keys, the unit a b-tree reads
/// (`BufferPool::DEFAULT_PAGE_SIZE`). Each kind is repeated, up to once per key, until its time
/// reaches the slowest kind's single run, so the clock's step weighs on each about equally.
fn calibrate(clock: &mut dyn FnMut() -> f64) -> Rates {
    let n = crate::storage::buffer_pool::BufferPool::DEFAULT_PAGE_SIZE / std::mem::size_of::<u64>();
    let keys: Vec<u64> = (0..n as u64).map(mix).collect();
    let mut ordered = keys.clone();
    ordered.sort_unstable();
    let table: FxHashMap<u64, u64> = keys.iter().map(|k| (*k, *k)).collect();
    let rows = n as f64;

    let mut row = || {
        black_box(keys.iter().filter(|k| black_box(**k) & 1 == 0).count());
    };
    let mut seek = || {
        for k in &keys {
            let _ = black_box(ordered.binary_search(k));
        }
    };
    let mut build = || {
        let mut built = FxHashMap::default();
        for k in &keys {
            built.insert(*k, *k);
        }
        black_box(&built);
    };
    let mut probe = || {
        for k in &keys {
            black_box(table.get(k));
        }
    };
    let mut sort = || {
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        black_box(&sorted);
    };
    let mut kinds: [(Work, f64, &mut dyn FnMut()); 5] = [
        (Work::Row, rows, &mut row),
        (Work::Seek, rows, &mut seek),
        (Work::HashBuild, rows, &mut build),
        (Work::HashProbe, rows, &mut probe),
        (Work::Sort, rows * rows.log2(), &mut sort),
    ];
    let mut target: f64 = 0.0;
    for (_, _, work) in kinds.iter_mut() {
        target = target.max(measure(clock, 0.0, 1, &mut **work).1);
    }
    let mut rates = Rates::default();
    for (kind, units, work) in kinds.iter_mut() {
        let (reps, nanos) = measure(clock, target, n, &mut **work);
        *rates.rate_mut(*kind) = Rate::calibrated(reps * *units, nanos);
    }
    // A page read can't be timed here: there's no file yet. Each database anchors it at open
    // (`time_page_read`, `LearnedCosts::anchor_page_read`), then its own reads move it
    // (`LearnedCosts::observe`), its epoch the pages whose rows calibration visited.
    rates.page = Rate {
        epoch: (rates.row.epoch / rows).max(1.0),
        ..Rate::default()
    };
    rates
}

/// Times one page read at a database's open, before it has read a page of its own (QT.5): a
/// scratch page of `page_size` bytes, written beside the database and read back. Nanoseconds,
/// or None if no scratch page could be written there (the database's own reads then give the
/// first time; a database whose directory takes no file is one opened read-only, whose open reads
/// its file).
///
/// A page read back right after it was written comes from the OS's cache, a read the database
/// rarely makes cold. So where the OS has an uncached read the scratch page is read with it:
/// `O_DIRECT` on Linux, `F_NOCACHE` on macOS and the other Apple systems, `FILE_FLAG_NO_BUFFERING`
/// on Windows, each with the page in a buffer aligned to its size. Where the OS or the file system
/// refuses one (Linux's tmpfs refuses `O_DIRECT`), and for a database in memory, the page is
/// written and read back through the database's own IO, as its pager reads: a cache-hot read for a
/// file, the read itself in memory. Either way the first epoch of the database's own reads
/// replaces it (`LearnedCosts::anchor_page_read`).
pub(crate) fn time_page_read(
    db_path: &str,
    io: &crate::sync::Arc<dyn crate::io::IO>,
    page_size: usize,
) -> Option<f64> {
    let page: Vec<u8> = (0..page_size)
        .map(|i| mix(i as u64) as u8)
        .collect();
    let in_memory = crate::database::is_memory_like(db_path);
    let scratch = scratch_path(db_path, in_memory);
    #[cfg(all(feature = "fs", not(target_family = "wasm")))]
    if !in_memory {
        match uncached_read(std::path::Path::new(&scratch), &page) {
            Ok(nanos) => return Some(nanos),
            Err(err) => tracing::debug!("QT.5: no uncached page read beside {db_path}: {err}"),
        }
    }
    match read_through_io(io, &scratch, page) {
        Ok(nanos) => Some(nanos),
        Err(err) => {
            tracing::debug!("QT.5: no scratch page read beside {db_path}: {err}");
            None
        }
    }
}

/// The scratch page's path: the database's own, with a suffix no other file shares.
fn scratch_path(db_path: &str, in_memory: bool) -> String {
    let unique = uuid::Uuid::new_v4().simple();
    if in_memory {
        format!(":memory:-page-read-{unique}")
    } else {
        format!("{db_path}-page-read-{unique}")
    }
}

/// Writes `page` to a new file at `scratch` and reads it back with the OS's uncached read; the
/// read's nanoseconds. The file is removed whatever happens.
#[cfg(all(feature = "fs", not(target_family = "wasm")))]
fn uncached_read(scratch: &std::path::Path, page: &[u8]) -> std::io::Result<f64> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_DIRECT);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_NO_BUFFERING);
    }
    #[cfg(not(any(target_os = "linux", target_vendor = "apple", windows)))]
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no uncached read on this OS",
        ));
    }
    let file = options.open(scratch)?;
    let timed = (|| -> std::io::Result<f64> {
        #[cfg(target_vendor = "apple")]
        rustix::fs::fcntl_nocache(&file, true)?;
        let mut written = vec![0u8; page.len() * 2];
        let out = aligned(&mut written, page.len())?;
        out.copy_from_slice(page);
        write_at(&file, out)?;
        let mut read = vec![0u8; page.len() * 2];
        let back = aligned(&mut read, page.len())?;
        let start = std::time::Instant::now();
        read_at(&file, back)?;
        let nanos = start.elapsed().as_nanos() as f64;
        if back != page {
            return Err(std::io::Error::other("the scratch page read back differs"));
        }
        Ok(nanos)
    })();
    drop(file);
    let removed = std::fs::remove_file(scratch);
    let nanos = timed?;
    removed?;
    Ok(nanos)
}

/// `len` bytes of `raw` starting at an address that is a multiple of `len` (a page size, a power
/// of two, which uncached reads need their buffers aligned to); `raw` holds at least `2 * len`.
#[cfg(all(feature = "fs", not(target_family = "wasm")))]
fn aligned(raw: &mut [u8], len: usize) -> std::io::Result<&mut [u8]> {
    let start = raw.as_ptr().align_offset(len);
    start
        .checked_add(len)
        .and_then(|end| raw.get_mut(start..end))
        .ok_or_else(|| std::io::Error::other("no aligned page in the scratch buffer"))
}

#[cfg(all(feature = "fs", unix))]
fn write_at(file: &std::fs::File, buf: &[u8]) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, buf, 0)
}

#[cfg(all(feature = "fs", unix))]
fn read_at(file: &std::fs::File, buf: &mut [u8]) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, 0)
}

#[cfg(all(feature = "fs", windows))]
fn write_at(file: &std::fs::File, buf: &[u8]) -> std::io::Result<()> {
    let n = std::os::windows::fs::FileExt::seek_write(file, buf, 0)?;
    if n != buf.len() {
        return Err(std::io::Error::other("short write of the scratch page"));
    }
    Ok(())
}

#[cfg(all(feature = "fs", windows))]
fn read_at(file: &std::fs::File, buf: &mut [u8]) -> std::io::Result<()> {
    let n = std::os::windows::fs::FileExt::seek_read(file, buf, 0)?;
    if n != buf.len() {
        return Err(std::io::Error::other("short read of the scratch page"));
    }
    Ok(())
}

/// Writes `page` to `scratch` through `io` and reads it back as the pager reads, from the read's
/// submission to its completion; the read's nanoseconds. The file is removed whatever happens.
fn read_through_io(
    io: &crate::sync::Arc<dyn crate::io::IO>,
    scratch: &str,
    page: Vec<u8>,
) -> crate::Result<f64> {
    use crate::io::{Buffer, Completion, OpenFlags};
    use crate::sync::Arc;
    let file = io.open_file(scratch, OpenFlags::Create, true)?;
    let timed = (|| -> crate::Result<f64> {
        let len = page.len();
        let written = file.pwrite(0, Arc::new(Buffer::new(page)), Completion::new_write(|_| {}))?;
        io.wait_for_completion(written)?;
        let start = std::time::Instant::now();
        let read = file.pread(
            0,
            Completion::new_read(Arc::new(Buffer::new(vec![0; len])), move |res| {
                match res {
                    Ok((_, n)) if n as usize == len => None,
                    Ok((_, n)) => Some(crate::error::CompletionError::ShortRead {
                        page_idx: 0,
                        expected: len,
                        actual: n.max(0) as usize,
                    }),
                    Err(err) => Some(err),
                }
            }),
        )?;
        io.wait_for_completion(read)?;
        Ok(start.elapsed().as_nanos() as f64)
    })();
    drop(file);
    let removed = io.remove_file(scratch);
    let nanos = timed?;
    removed?;
    Ok(nanos)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_rates(row: f64, seek: f64, build: f64, probe: f64, sort: f64) -> Rates {
        Rates {
            row: Rate::calibrated(100.0, 100.0 * row),
            seek: Rate::calibrated(100.0, 100.0 * seek),
            hash_build: Rate::calibrated(100.0, 100.0 * build),
            hash_probe: Rate::calibrated(100.0, 100.0 * probe),
            sort: Rate::calibrated(100.0, 100.0 * sort),
            page: Rate {
                epoch: 4.0,
                ..Rate::default()
            },
        }
    }

    /// Do: calibrate on this machine with no operator observed. Expect: every weight a finite,
    /// positive number of rows' visits (T-QT.5.5: the learned costs start sane).
    #[test]
    fn t_qt_5_5_calibration_starts_sane_with_no_observations() {
        let weights = LearnedCosts::new().weights().expect("the clock moved");
        for w in [
            weights.seek,
            weights.hash_build,
            weights.hash_probe,
            weights.sort,
        ] {
            assert!(w.is_finite() && w > 0.0, "{weights:?}");
        }
    }

    /// Do: start from known rates, then observe sorts at four times the calibrated rate for as
    /// much work as two epochs. Expect: the sort weight moves to four times its start, the others
    /// stay (T-QT.5.5: the estimator follows observed times).
    #[test]
    fn t_qt_5_5_observed_times_move_the_weight() {
        let costs = LearnedCosts::from_rates(fixed_rates(1.0, 10.0, 20.0, 15.0, 2.0));
        let start = costs.weights().unwrap();
        assert_eq!(start.sort, 2.0);
        costs.observe(Work::Sort, 50.0, 50.0 * 8.0);
        let partway = costs.weights().unwrap();
        assert!(partway.sort > 2.0 && partway.sort < 8.0, "{partway:?}");
        for _ in 0..4 {
            costs.observe(Work::Sort, 50.0, 50.0 * 8.0);
        }
        let moved = costs.weights().unwrap();
        assert_eq!(moved.sort, 8.0);
        assert_eq!(moved.seek, start.seek);
        assert_eq!(moved.hash_build, start.hash_build);
        assert_eq!(moved.hash_probe, start.hash_probe);
    }

    /// Do: observe nothing-sized and non-finite times. Expect: the weights unchanged (T-QT.5.5).
    #[test]
    fn t_qt_5_5_meaningless_observations_leave_the_rates() {
        let costs = LearnedCosts::from_rates(fixed_rates(1.0, 10.0, 20.0, 15.0, 2.0));
        let start = costs.weights().unwrap();
        costs.observe(Work::Seek, 0.0, 5.0);
        costs.observe(Work::Seek, 10.0, f64::NAN);
        costs.observe(Work::Seek, 10.0, -1.0);
        assert_eq!(costs.weights().unwrap(), start);
    }

    /// Do: a slower row visit. Expect: every weight, counted in rows' visits, falls in proportion
    /// (T-QT.5.5: the weights are relative to the row).
    #[test]
    fn t_qt_5_5_weights_are_in_rows_visits() {
        let fast = LearnedCosts::from_rates(fixed_rates(1.0, 10.0, 20.0, 15.0, 2.0));
        let slow = LearnedCosts::from_rates(fixed_rates(2.0, 10.0, 20.0, 15.0, 2.0));
        let (fast, slow) = (fast.weights().unwrap(), slow.weights().unwrap());
        assert_eq!(slow.seek * 2.0, fast.seek);
        assert_eq!(slow.sort * 2.0, fast.sort);
    }

    /// Do: plan before the database read any page from its file, then after it timed page reads at
    /// 50 rows' visits each, then after reads turn four times slower for two epochs. Expect: no
    /// row-to-page weight at first, then a fiftieth, then a two-hundredth (the anchor learned where
    /// it runs, and followed as the device moves).
    #[test]
    fn t_qt_5_5_the_row_is_weighed_against_observed_page_reads() {
        let costs = LearnedCosts::from_rates(fixed_rates(2.0, 10.0, 20.0, 15.0, 2.0));
        assert_eq!(costs.weights().unwrap().row_in_pages, None);
        costs.observe(Work::PageRead, 1.0, 100.0);
        assert_eq!(costs.weights().unwrap().row_in_pages, Some(2.0 / 100.0));
        for _ in 0..8 {
            costs.observe(Work::PageRead, 1.0, 400.0);
        }
        assert_eq!(costs.weights().unwrap().row_in_pages, Some(2.0 / 400.0));
    }

    /// Do: the weights with no page read timed, and with one, applied to the planner's parameters.
    /// Expect: the row's cost stays the parameters' until a page read is timed, then is the
    /// learned ratio, and every other weight follows it (T-QT.5.5).
    #[test]
    fn t_qt_5_5_the_planner_takes_the_learned_row_cost() {
        let params = super::super::cost_params::CostModelParams::new();
        let costs = LearnedCosts::from_rates(fixed_rates(2.0, 10.0, 20.0, 15.0, 2.0));
        let before = params.with_learned(costs.weights());
        assert_eq!(before.cpu_cost_per_row, params.cpu_cost_per_row);
        costs.observe(Work::PageRead, 1.0, 128.0);
        let after = params.with_learned(costs.weights());
        assert_eq!(after.cpu_cost_per_row, 2.0 / 128.0);
        assert_eq!(after.cpu_cost_per_seek, 10.0 / 128.0);
        assert_eq!(after.hash_insert_cost, 20.0 / 128.0);
    }

    /// Do: anchor the page read at 100 ns, as a database's open does, then read pages of its own
    /// at 400 ns each for one epoch. Expect: the row weighed against the anchor at first, then
    /// against the database's own reads alone (the anchor replaced, no fixed number left).
    #[test]
    fn t_qt_5_5_the_open_anchor_stands_until_the_database_reads() {
        let costs = LearnedCosts::from_rates(fixed_rates(2.0, 10.0, 20.0, 15.0, 2.0));
        costs.anchor_page_read(100.0);
        assert_eq!(costs.weights().unwrap().row_in_pages, Some(2.0 / 100.0));
        for _ in 0..4 {
            costs.observe(Work::PageRead, 1.0, 400.0);
        }
        assert_eq!(costs.weights().unwrap().row_in_pages, Some(2.0 / 400.0));
    }

    /// Do: time a scratch page read beside a new database file. Expect: a time, and the
    /// directory left as it was (the scratch page removed).
    #[cfg(feature = "fs")]
    #[test]
    fn t_qt_5_5_a_scratch_page_read_is_timed_beside_the_database() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("anchor.db");
        let io: crate::sync::Arc<dyn crate::io::IO> =
            crate::sync::Arc::new(crate::PlatformIO::new().unwrap());
        let nanos = time_page_read(path.to_str().unwrap(), &io, 4096).unwrap();
        assert!(nanos.is_finite() && nanos > 0.0, "{nanos}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    /// Do: time a scratch page read for a database in memory. Expect: a time, read through its
    /// own IO.
    #[test]
    fn t_qt_5_5_a_scratch_page_read_is_timed_in_memory() {
        let io: crate::sync::Arc<dyn crate::io::IO> =
            crate::sync::Arc::new(crate::MemoryIO::new());
        let nanos = time_page_read(":memory:", &io, 4096).unwrap();
        assert!(nanos.is_finite() && nanos > 0.0, "{nanos}");
    }

    /// Do: open a new, empty database file and ask for its weights before any query. Expect: the
    /// row already weighed in page reads (the open's anchor), not left to a fixed number.
    #[cfg(feature = "fs")]
    #[test]
    fn t_qt_5_5_a_new_database_has_a_page_read_before_its_first_query() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("new.db");
        let io: crate::sync::Arc<dyn crate::io::IO> =
            crate::sync::Arc::new(crate::PlatformIO::new().unwrap());
        let db = crate::Database::open_file(
            io,
            path.to_str().unwrap(),
            crate::sync::Arc::new(crate::SqliteDialect),
        )
        .unwrap();
        let row = db.learned_costs.weights().unwrap().row_in_pages.unwrap();
        assert!(row.is_finite() && row > 0.0, "{row}");
    }

    /// Do: calibrate with a clock that never moves. Expect: no weights, so the planner is told it
    /// has none rather than given zeros (T-QT.5.5).
    #[test]
    fn t_qt_5_5_a_stopped_clock_gives_no_weights() {
        let rates = calibrate(&mut || 0.0);
        assert_eq!(rates.weights(), None);
    }
}
