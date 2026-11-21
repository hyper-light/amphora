//! Amphora: the versioned storage's buffer pool (Amphora's `docs/design/engine.md`, decision 2).
//!
//! A fixed number of frames, sized by its budget, each holding one page behind a latch (shared to
//! read, exclusive to write) and a version counter that every exclusive hold bumps as it ends.
//! Readers that keep a copy of a page (the inner pages a B-link descent reads, decision 3) know it
//! is current while the counter they load still matches the one they copied it under: a load, a
//! read of a line no reader writes.
//!
//! Every page is resident and page `n` lives in frame `n - 1`: a page number is its frame, with no
//! table to look it up in. A page asked for past the frames is refused (`DatabaseFull`), as is a
//! new page once every frame holds one. Eviction to the file, with a table from pages to frames,
//! comes with the storage's durability (decisions 6 and 7).
//!
//! Checkpoints copy pages by copy-on-update (decision 7): writes fall in epochs, a checkpoint's
//! pause ends one ([`Pool::begin_copy`]), and the checkpoint then copies the pages written in it as
//! they were at the pause ([`Pool::copy_owed`]), each under its shared latch, while writers go on:
//! a writer that reaches such a page first keeps its bytes as they were before it changes them.

use super::page::PAGE_SIZE;
use crate::error::LimboError;
use crate::sync::atomic::{AtomicU64, Ordering};
use crate::sync::{Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use crate::Result;
use std::ops::{Deref, DerefMut};

/// A page's frame.
struct Frame {
    page: RwLock<Box<[u8]>>,
    /// Exclusive holds ended.
    version: AtomicU64,
    /// The epoch of its last write (0: none since it was loaded).
    written: AtomicU64,
    /// The last epoch a checkpoint has its bytes of.
    copied: AtomicU64,
    /// Its bytes at the pause of the checkpoint copying now, kept by the first write after it: at
    /// most one page a frame.
    kept: Mutex<Option<Box<[u8]>>>,
}

/// The frames, and the next page number to hand out.
pub struct Pool {
    frames: Vec<Frame>,
    /// Pages handed out: page numbers `1..=allocated`.
    allocated: AtomicU64,
    /// The epoch writes fall in now: moved on only by a checkpoint's pause, when no page is
    /// written.
    epoch: AtomicU64,
    /// The epoch a checkpoint is copying (0: none).
    copying: AtomicU64,
}

/// A page read under its frame's shared latch.
pub struct PageRead<'a> {
    guard: RwLockReadGuard<'a, Box<[u8]>>,
    version: u64,
}

impl PageRead<'_> {
    /// The frame's version while this hold lasts: what a copy taken now is current for.
    pub fn version(&self) -> u64 {
        self.version
    }
}

impl Deref for PageRead<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.guard
    }
}

/// A page written under its frame's exclusive latch; its frame's version moves on as it ends, and
/// it is written in the epoch it began in, for the checkpoint that ends that epoch to copy.
pub struct PageWrite<'a> {
    guard: RwLockWriteGuard<'a, Box<[u8]>>,
    frame: &'a Frame,
    epoch: u64,
}

impl Deref for PageWrite<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.guard
    }
}

impl DerefMut for PageWrite<'_> {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.guard
    }
}

impl Drop for PageWrite<'_> {
    fn drop(&mut self) {
        self.frame.written.store(self.epoch, Ordering::Release);
        self.frame.version.fetch_add(1, Ordering::Release);
    }
}

impl Pool {
    /// A pool of `frames` pages, all zeroed.
    pub fn new(frames: usize) -> Self {
        let frames = (0..frames)
            .map(|_| Frame {
                page: RwLock::new(vec![0u8; PAGE_SIZE].into_boxed_slice()),
                version: AtomicU64::new(0),
                written: AtomicU64::new(0),
                copied: AtomicU64::new(0),
                kept: Mutex::new(None),
            })
            .collect();
        Self {
            frames,
            allocated: AtomicU64::new(0),
            epoch: AtomicU64::new(1),
            copying: AtomicU64::new(0),
        }
    }

    /// The pages the pool holds at most.
    pub fn capacity(&self) -> usize {
        self.frames.len()
    }

    /// The pages handed out.
    pub fn allocated(&self) -> u64 {
        self.allocated.load(Ordering::Acquire)
    }

    /// A new page's number, its frame zeroed; `DatabaseFull` once every frame holds a page. Each
    /// failed exchange means another caller took a page, so the loop ends within the capacity.
    pub fn allocate(&self) -> Result<u64> {
        let capacity = u64::try_from(self.frames.len()).map_err(|_| LimboError::IntegerOverflow)?;
        let mut n = self.allocated.load(Ordering::Acquire);
        loop {
            if n >= capacity {
                return Err(LimboError::DatabaseFull);
            }
            let next = n.checked_add(1).ok_or(LimboError::IntegerOverflow)?;
            match self
                .allocated
                .compare_exchange(n, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return Ok(next),
                Err(now) => n = now,
            }
        }
    }

    fn frame(&self, page: u64) -> Result<&Frame> {
        if page == 0 || page > self.allocated() {
            return Err(LimboError::Corrupt(format!(
                "versioned storage: page {page} was never handed out"
            )));
        }
        let at =
            usize::try_from(page.saturating_sub(1)).map_err(|_| LimboError::IntegerOverflow)?;
        self.frames.get(at).ok_or_else(|| {
            LimboError::Corrupt(format!("versioned storage: page {page} has no frame"))
        })
    }

    /// `page`, latched shared.
    pub fn read(&self, page: u64) -> Result<PageRead<'_>> {
        let frame = self.frame(page)?;
        let guard = frame.page.read();
        let version = frame.version.load(Ordering::Acquire);
        Ok(PageRead { guard, version })
    }

    /// `page`, latched exclusive. The first write to a page the checkpoint copying now still owes
    /// keeps its bytes first, as they were at the checkpoint's pause.
    pub fn write(&self, page: u64) -> Result<PageWrite<'_>> {
        let frame = self.frame(page)?;
        let guard = frame.page.write();
        let copying = self.copying.load(Ordering::Acquire);
        if copying != 0
            && frame.written.load(Ordering::Acquire) == copying
            && frame.copied.load(Ordering::Acquire) != copying
        {
            *frame.kept.lock() = Some(guard.clone());
            frame.copied.store(copying, Ordering::Release);
        }
        Ok(PageWrite {
            guard,
            frame,
            epoch: self.epoch.load(Ordering::Acquire),
        })
    }

    /// A checkpoint's pause (call while no page can be written): ends the epoch writes fell in, and
    /// returns it, for [`Pool::copy_owed`] to copy the pages written in it as they are now.
    pub fn begin_copy(&self) -> Result<u64> {
        let ending = self.epoch.load(Ordering::Acquire);
        let next = ending.checked_add(1).ok_or(LimboError::IntegerOverflow)?;
        self.copying.store(ending, Ordering::Release);
        self.epoch.store(next, Ordering::Release);
        Ok(ending)
    }

    /// The pages written in `epoch` (as [`Pool::begin_copy`] ended it) among the first `pages`
    /// handed out, each with its bytes as they were when it ended, while writers go on: a page
    /// none wrote since, copied now under its shared latch; one written since, as its first
    /// writer kept it. The copying ends with it.
    pub fn copy_owed(&self, epoch: u64, pages: u64) -> Result<Vec<(u64, Vec<u8>)>> {
        let mut out = Vec::new();
        let pages = usize::try_from(pages).map_err(|_| LimboError::IntegerOverflow)?;
        for (i, frame) in self.frames.iter().take(pages).enumerate() {
            let page = u64::try_from(i)
                .ok()
                .and_then(|i| i.checked_add(1))
                .ok_or(LimboError::IntegerOverflow)?;
            let held = frame.page.read();
            if frame.copied.load(Ordering::Acquire) == epoch {
                if let Some(kept) = frame.kept.lock().take() {
                    out.push((page, kept.into_vec()));
                }
            } else if frame.written.load(Ordering::Acquire) == epoch {
                frame.copied.store(epoch, Ordering::Release);
                out.push((page, held.to_vec()));
            }
        }
        self.copying.store(0, Ordering::Release);
        Ok(out)
    }

    /// Puts `bytes`, page `page` as a file holds it, into its frame (recovery), clean; the pages
    /// handed out cover it after.
    pub fn load(&self, page: u64, bytes: &[u8]) -> Result<()> {
        let at = page
            .checked_sub(1)
            .and_then(|i| usize::try_from(i).ok())
            .ok_or(LimboError::IntegerOverflow)?;
        let frame = self.frames.get(at).ok_or(LimboError::DatabaseFull)?;
        let mut w = frame.page.write();
        if bytes.len() != w.len() {
            return Err(LimboError::Corrupt(format!(
                "versioned storage: page {page} of {} bytes",
                bytes.len()
            )));
        }
        w.copy_from_slice(bytes);
        drop(w);
        frame.version.fetch_add(1, Ordering::Release);
        self.allocated.fetch_max(page, Ordering::AcqRel);
        Ok(())
    }

    /// `page`'s frame's version now, unlatched.
    pub fn version(&self, page: u64) -> Result<u64> {
        Ok(self.frame(page)?.version.load(Ordering::Acquire))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Do: hand out every frame and one more; read and write a page. Expect: pages 1 to the
    /// capacity, then `DatabaseFull`; a write's end moves the frame's version, a read's doesn't;
    /// a page never handed out is refused.
    #[test]
    fn pages_are_handed_out_to_the_bound_and_writes_move_versions() {
        let pool = Pool::new(3);
        assert_eq!(
            (1..=3)
                .map(|_| pool.allocate().unwrap())
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert!(matches!(pool.allocate(), Err(LimboError::DatabaseFull)));
        let before = pool.version(2).unwrap();
        {
            let mut w = pool.write(2).unwrap();
            w[0] = 7;
        }
        assert_eq!(pool.version(2).unwrap(), before + 1);
        let r = pool.read(2).unwrap();
        assert_eq!((r[0], r.version()), (7, before + 1));
        drop(r);
        assert_eq!(pool.version(2).unwrap(), before + 1);
        assert!(pool.read(0).is_err() && Pool::new(3).read(1).is_err());
    }
}
