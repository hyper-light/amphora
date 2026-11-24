use crate::sync::atomic::{AtomicBool, Ordering};
use crate::thread::spin_loop;
use std::{
    cell::UnsafeCell,
    ops::{Deref, DerefMut},
};

#[derive(Debug)]
pub struct SpinLock<T: ?Sized> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.value.get() }
    }
}

unsafe impl<T: ?Sized + Send> Send for SpinLock<T> {}
unsafe impl<T: ?Sized + Send> Sync for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        // Amphora: a waiter spins on a plain load and swaps only once the lock looks free, so
        // waiters share the line instead of taking it from each other at every turn (Anderson,
        // "The Performance of Spin Lock Alternatives for Shared-Memory Multiprocessors", IEEE
        // TPDS 1990: test-and-test-and-set; store.md decision 28).
        loop {
            if !self.locked.swap(true, Ordering::Acquire) {
                return SpinLockGuard { lock: self };
            }
            while self.locked.load(Ordering::Relaxed) {
                spin_loop();
            }
        }
    }

    pub fn into_inner(self) -> UnsafeCell<T> {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use crate::sync::Arc;

    use super::SpinLock;

    #[test]
    fn test_fast_lock_multiple_thread_sum() {
        let lock = Arc::new(SpinLock::new(0));
        let mut threads = vec![];
        const NTHREADS: usize = 1000;
        for _ in 0..NTHREADS {
            let lock = lock.clone();
            threads.push(std::thread::spawn(move || {
                let mut guard = lock.lock();
                *guard += 1;
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(*lock.lock(), NTHREADS);
    }
}

#[cfg(all(shuttle, test))]
mod shuttle_tests {
    use super::*;
    use crate::sync::*;
    use crate::thread;

    /// Test basic mutual exclusion with counter increment
    #[test]
    fn shuttle_spinlock_counter() {
        shuttle::check_random(
            || {
                let lock = Arc::new(SpinLock::new(0));
                let mut threads = vec![];
                const NTHREADS: usize = 3;
                for _ in 0..NTHREADS {
                    let lock = lock.clone();
                    threads.push(thread::spawn(move || {
                        let mut guard = lock.lock();
                        *guard += 1;
                    }));
                }
                for thread in threads {
                    thread.join().unwrap();
                }
                assert_eq!(*lock.lock(), NTHREADS);
            },
            1000,
        );
    }

    /// Test that lock provides mutual exclusion - no two threads hold lock simultaneously
    #[test]
    fn shuttle_spinlock_mutual_exclusion() {
        shuttle::check_random(
            || {
                let lock = Arc::new(SpinLock::new(()));
                let in_critical_section = Arc::new(AtomicBool::new(false));

                let mut threads = vec![];
                for _ in 0..3 {
                    let lock = lock.clone();
                    let in_cs = in_critical_section.clone();
                    threads.push(thread::spawn(move || {
                        let _guard = lock.lock();
                        // If another thread is in critical section, this is a bug
                        assert!(
                            !in_cs.swap(true, Ordering::SeqCst),
                            "Two threads in critical section!"
                        );
                        // Simulate some work
                        thread::yield_now();
                        in_cs.store(false, Ordering::SeqCst);
                    }));
                }
                for thread in threads {
                    thread.join().unwrap();
                }
            },
            1000,
        );
    }

    /// Test multiple lock/unlock cycles per thread
    #[test]
    fn shuttle_spinlock_multiple_cycles() {
        shuttle::check_random(
            || {
                let lock = Arc::new(SpinLock::new(0i32));

                let mut threads = vec![];
                for _ in 0..2 {
                    let lock = lock.clone();
                    threads.push(thread::spawn(move || {
                        for _ in 0..3 {
                            let mut guard = lock.lock();
                            *guard += 1;
                            // Guard dropped here, releasing lock
                        }
                    }));
                }
                for thread in threads {
                    thread.join().unwrap();
                }
                // 2 threads * 3 iterations = 6
                assert_eq!(*lock.lock(), 6);
            },
            1000,
        );
    }

    /// Test that guard properly releases lock on drop
    #[test]
    fn shuttle_spinlock_guard_drop() {
        shuttle::check_random(
            || {
                let lock = Arc::new(SpinLock::new(0));

                let lock1 = lock.clone();
                let t1 = thread::spawn(move || {
                    {
                        let mut guard = lock1.lock();
                        *guard = 1;
                        // guard dropped here
                    }
                    // After drop, another thread should be able to acquire
                });

                let lock2 = lock.clone();
                let t2 = thread::spawn(move || {
                    let mut guard = lock2.lock();
                    *guard = 2;
                });

                t1.join().unwrap();
                t2.join().unwrap();

                // Value should be 1 or 2 depending on order, but lock should be acquirable
                let val = *lock.lock();
                assert!(val == 1 || val == 2);
            },
            1000,
        );
    }

    /// Test read-modify-write pattern under contention
    #[test]
    fn shuttle_spinlock_read_modify_write() {
        shuttle::check_random(
            || {
                let lock = Arc::new(SpinLock::new(vec![0i32; 3]));

                let mut threads = vec![];
                for i in 0..3 {
                    let lock = lock.clone();
                    threads.push(thread::spawn(move || {
                        let mut guard = lock.lock();
                        // Read current value, modify, write back
                        guard[i] += 1;
                    }));
                }
                for thread in threads {
                    thread.join().unwrap();
                }
                let guard = lock.lock();
                assert_eq!(*guard, vec![1, 1, 1]);
            },
            1000,
        );
    }

    /// Test lock acquisition order doesn't cause starvation (probabilistic)
    #[test]
    fn shuttle_spinlock_no_starvation() {
        shuttle::check_random(
            || {
                let lock = Arc::new(SpinLock::new(Vec::<usize>::new()));

                let mut threads = vec![];
                for id in 0..3 {
                    let lock = lock.clone();
                    threads.push(thread::spawn(move || {
                        let mut guard = lock.lock();
                        guard.push(id);
                    }));
                }
                for thread in threads {
                    thread.join().unwrap();
                }
                // All threads should have acquired the lock exactly once
                let guard = lock.lock();
                assert_eq!(guard.len(), 3);
                let mut sorted = guard.clone();
                sorted.sort();
                assert_eq!(sorted, vec![0, 1, 2]);
            },
            1000,
        );
    }

    /// Test nested-style access pattern (reacquire after release)
    #[test]
    fn shuttle_spinlock_reacquire() {
        shuttle::check_random(
            || {
                let lock = Arc::new(SpinLock::new(0));

                let lock1 = lock.clone();
                let t1 = thread::spawn(move || {
                    {
                        let mut guard = lock1.lock();
                        *guard += 1;
                    }
                    // Release and reacquire
                    {
                        let mut guard = lock1.lock();
                        *guard += 1;
                    }
                });

                let lock2 = lock.clone();
                let t2 = thread::spawn(move || {
                    let mut guard = lock2.lock();
                    *guard += 10;
                });

                t1.join().unwrap();
                t2.join().unwrap();

                // Should be 12 (1 + 1 + 10)
                assert_eq!(*lock.lock(), 12);
            },
            1000,
        );
    }
}

/// Lane Q8's diagnostic, not for main: a `SpinLock` that keeps, for each call site of `lock`, how long
/// its callers waited for the lock and how long they held it, and for each wait past 10 us the site that
/// last took it. `AMPHORA_FRAME_DIAG=<path>` writes the table there as the process exits.
pub mod diag {
    use super::{SpinLock, SpinLockGuard};
    use std::collections::HashMap;
    use std::io::Write;
    use std::ops::{Deref, DerefMut};
    use std::panic::Location;
    use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    const SITES: usize = 4096;
    const BUCKETS: usize = 40;
    const LONG_NS: u64 = 10_000;

    struct Site {
        loc: AtomicPtr<Location<'static>>,
        acq: AtomicU64,
        wait_ns: AtomicU64,
        wait_max: AtomicU64,
        hold_ns: AtomicU64,
        hold_max: AtomicU64,
        wait_hist: [AtomicU64; BUCKETS],
        hold_hist: [AtomicU64; BUCKETS],
    }

    struct Table {
        sites: Vec<Site>,
        blame: Mutex<HashMap<(usize, usize), (u64, u64)>>,
        extra: [(AtomicU64, AtomicU64, AtomicU64); 8],
    }

    static TABLE: OnceLock<Table> = OnceLock::new();
    static HOLDER: AtomicUsize = AtomicUsize::new(0);

    fn bucket(ns: u64) -> usize {
        (64usize.saturating_sub(ns.max(1).leading_zeros() as usize)).min(BUCKETS - 1)
    }

    fn table() -> &'static Table {
        TABLE.get_or_init(|| {
            if std::env::var_os("AMPHORA_FRAME_DIAG").is_some() {
                #[cfg(unix)]
                unsafe {
                    libc::atexit(dump);
                }
            }
            Table {
                sites: (0..SITES)
                    .map(|_| Site {
                        loc: AtomicPtr::new(std::ptr::null_mut()),
                        acq: AtomicU64::new(0),
                        wait_ns: AtomicU64::new(0),
                        wait_max: AtomicU64::new(0),
                        hold_ns: AtomicU64::new(0),
                        hold_max: AtomicU64::new(0),
                        wait_hist: std::array::from_fn(|_| AtomicU64::new(0)),
                        hold_hist: std::array::from_fn(|_| AtomicU64::new(0)),
                    })
                    .collect(),
                blame: Mutex::new(HashMap::new()),
                extra: std::array::from_fn(|_| (AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0))),
            }
        })
    }

    pub fn extra(which: usize, ns: u64) {
        let Some((n, total, max)) = table().extra.get(which) else { return };
        n.fetch_add(1, Ordering::Relaxed);
        total.fetch_add(ns, Ordering::Relaxed);
        max.fetch_max(ns, Ordering::Relaxed);
    }

    extern "C" fn dump() {
        let Some(path) = std::env::var_os("AMPHORA_FRAME_DIAG") else { return };
        let t = table();
        let mut out = String::new();
        for s in &t.sites {
            let n = s.acq.load(Ordering::Relaxed);
            if n == 0 {
                continue;
            }
            let loc = s.loc.load(Ordering::Relaxed);
            let name = if loc.is_null() {
                "?".to_string()
            } else {
                let l = unsafe { &*loc };
                format!("{}:{}", l.file().rsplit('/').next().unwrap_or(""), l.line())
            };
            let (w, wm, h, hm) = (s.wait_ns.load(Ordering::Relaxed), s.wait_max.load(Ordering::Relaxed), s.hold_ns.load(Ordering::Relaxed), s.hold_max.load(Ordering::Relaxed));
            let hist = |a: &[AtomicU64; BUCKETS]| {
                a.iter().enumerate().filter(|(_, c)| c.load(Ordering::Relaxed) > 0).map(|(i, c)| format!("2^{}:{}", i, c.load(Ordering::Relaxed))).collect::<Vec<_>>().join(",")
            };
            out.push_str(&format!(
                "site {name} acq {n} wait_ms {:.1} wait_mean_ns {} wait_max_us {} hold_ms {:.1} hold_mean_ns {} hold_max_us {} wait_hist [{}] hold_hist [{}]\n",
                w as f64 / 1e6, w / n, wm / 1000, h as f64 / 1e6, h / n, hm / 1000, hist(&s.wait_hist), hist(&s.hold_hist)
            ));
        }
        if let Ok(b) = t.blame.lock() {
            let mut rows: Vec<_> = b.iter().collect();
            rows.sort_by_key(|(_, v)| std::cmp::Reverse(v.1));
            for ((w, h), (n, ns)) in rows.into_iter().take(40) {
                out.push_str(&format!("blame: waiter site {w} waited on holder site {h}: long waits {n} total_ms {:.1}\n", *ns as f64 / 1e6));
            }
        }
        let mut names = String::new();
        for (i, (n, total, max)) in t.extra.iter().enumerate() {
            let (n, total, max) = (n.load(Ordering::Relaxed), total.load(Ordering::Relaxed), max.load(Ordering::Relaxed));
            if n > 0 {
                names.push_str(&format!("extra[{i}] n {n} total_ms {:.1} max_us {}\n", total as f64 / 1e6, max / 1000));
            }
        }
        out.push_str(&names);
        // Site index -> location, for reading the blame rows.
        for (i, s) in t.sites.iter().enumerate() {
            let loc = s.loc.load(Ordering::Relaxed);
            if !loc.is_null() {
                let l = unsafe { &*loc };
                out.push_str(&format!("index {i} = {}:{}\n", l.file().rsplit('/').next().unwrap_or(""), l.line()));
            }
        }
        if let Ok(mut f) = std::fs::File::create(&path) {
            let _ = f.write_all(out.as_bytes());
        }
    }

    #[derive(Debug)]
    pub struct DiagLock<T> {
        inner: SpinLock<T>,
    }

    pub struct DiagGuard<'a, T> {
        guard: Option<SpinLockGuard<'a, T>>,
        site: usize,
        start: Instant,
    }

    impl<T> DiagLock<T> {
        pub fn new(value: T) -> Self {
            Self { inner: SpinLock::new(value) }
        }

        #[track_caller]
        pub fn lock(&self) -> DiagGuard<'_, T> {
            let loc = Location::caller();
            let addr = loc as *const Location as usize;
            let t = table();
            let site = ((addr >> 4) ^ (addr >> 15)) % SITES;
            let Some(s) = t.sites.get(site) else {
                return DiagGuard { guard: Some(self.inner.lock()), site: 0, start: Instant::now() };
            };
            s.loc.store(loc as *const Location as *mut Location, Ordering::Relaxed);
            let begin = Instant::now();
            let holder = HOLDER.load(Ordering::Relaxed);
            let guard = self.inner.lock();
            let waited = begin.elapsed().as_nanos() as u64;
            HOLDER.store(site, Ordering::Relaxed);
            s.acq.fetch_add(1, Ordering::Relaxed);
            s.wait_ns.fetch_add(waited, Ordering::Relaxed);
            s.wait_max.fetch_max(waited, Ordering::Relaxed);
            if let Some(b) = s.wait_hist.get(bucket(waited)) {
                b.fetch_add(1, Ordering::Relaxed);
            }
            if waited > LONG_NS {
                if let Ok(mut b) = t.blame.try_lock() {
                    let e = b.entry((site, holder)).or_insert((0, 0));
                    e.0 += 1;
                    e.1 += waited;
                }
            }
            DiagGuard { guard: Some(guard), site, start: Instant::now() }
        }
    }

    impl<T> Drop for DiagGuard<'_, T> {
        fn drop(&mut self) {
            let held = self.start.elapsed().as_nanos() as u64;
            drop(self.guard.take());
            if let Some(s) = table().sites.get(self.site) {
                s.hold_ns.fetch_add(held, Ordering::Relaxed);
                s.hold_max.fetch_max(held, Ordering::Relaxed);
                if let Some(b) = s.hold_hist.get(bucket(held)) {
                    b.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    impl<T> Deref for DiagGuard<'_, T> {
        type Target = T;
        fn deref(&self) -> &T {
            match &self.guard {
                Some(g) => &**g,
                None => unreachable!(),
            }
        }
    }

    impl<T> DerefMut for DiagGuard<'_, T> {
        fn deref_mut(&mut self) -> &mut T {
            match &mut self.guard {
                Some(g) => &mut **g,
                None => unreachable!(),
            }
        }
    }
}
