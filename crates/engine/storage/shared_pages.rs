//! Amphora: the pages a database's connections share (Amphora's `docs/design/store.md`, decision 20).
//!
//! Each connection keeps a page cache of its own; a page it misses there is looked up here before
//! it is read from the file. What is kept is a page's image as committed, under the version it is
//! (a frame of the log, in the log's generation, or the database file's), so whichever connection
//! read it in first, every connection whose snapshot sees that version reads the same buffer: no
//! read, no checksum, no copy. The images never change: a frame's is committed (only a connection
//! that holds no write lock shares, so no frame of a write in progress is ever here), and a file
//! version is dropped before the database file's page is written or the file cut. A connection
//! that writes a shared buffer copies it first (`Pager::add_dirty`).
//!
//! Bounded: at most the pages it was made for, kept by CLOCK in shards by page number, each shard a
//! slice of the bound; a page added to a full shard evicts the first page the hand finds not read
//! since it last passed. One pool can serve many databases, a budget for them all: each file that
//! joins it gets a number of its own, part of its pages' keys, so no two files' pages meet.

use crate::io::Buffer;
use crate::sync::atomic::{AtomicU64, Ordering};
use crate::sync::{Arc, Mutex};
use rustc_hash::FxHashMap;

/// Shards: a lock each, so readers of different pages seldom meet.
const SHARDS: usize = 64;
const SHARDS_U64: u64 = 64;

/// Which version of a page an image is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Version {
    /// The image a frame of the log holds. A generation's frames number from 1 again after the
    /// log restarts, so the generation (the header's `checkpoint_seq`) is part of the version.
    Frame { generation: u32, frame: u64 },
    /// The database file's.
    File,
}

/// A page of one database file and the version of it an image is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Key {
    /// The file it is a page of: the number the pool gave the file ([`SharedPages::join`]).
    pub file: u64,
    pub page: u64,
    pub version: Version,
}

struct Slot {
    key: Key,
    buffer: Arc<Buffer>,
    /// Read since the hand last passed.
    visited: bool,
}

#[derive(Default)]
struct Shard {
    map: FxHashMap<Key, usize>,
    slots: Vec<Option<Slot>>,
    hand: usize,
}

/// Lookups that found an image, lookups that didn't, and images added: for tests and meters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub hits: u64,
    pub misses: u64,
    pub added: u64,
}

/// The pages a database's connections share.
pub struct SharedPages {
    shards: Box<[Mutex<Shard>]>,
    /// Pages each shard keeps at most.
    per_shard: usize,
    hits: AtomicU64,
    misses: AtomicU64,
    added: AtomicU64,
    /// The number the next file to join gets.
    files: AtomicU64,
}

impl SharedPages {
    /// Shares at most `pages` pages (at least one a shard).
    pub fn new(pages: usize) -> Self {
        let per_shard = pages.checked_div(SHARDS).unwrap_or(0).max(1);
        Self {
            shards: (0..SHARDS).map(|_| Mutex::new(Shard::default())).collect(),
            per_shard,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            added: AtomicU64::new(0),
            files: AtomicU64::new(0),
        }
    }

    /// A number for one more database file reading through these pages, its keys' `file`: every
    /// file that joins gets its own.
    pub fn join(&self) -> u64 {
        self.files.fetch_add(1, Ordering::Relaxed)
    }

    fn shard(&self, page: u64) -> Option<&Mutex<Shard>> {
        let at = page.checked_rem(SHARDS_U64).unwrap_or(0);
        self.shards.get(usize::try_from(at).ok()?)
    }

    /// The image of `key`, if one is kept.
    pub fn get(&self, key: &Key) -> Option<Arc<Buffer>> {
        let found = self.shard(key.page).and_then(|shard| {
            let mut shard = shard.lock();
            let at = *shard.map.get(key)?;
            let slot = shard.slots.get_mut(at)?.as_mut()?;
            slot.visited = true;
            Some(slot.buffer.clone())
        });
        if found.is_some() {
            self.hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
        }
        found
    }

    /// Keeps `buffer` as the image of `key`, evicting by CLOCK if its shard is full. An image
    /// already kept under `key` stays (both are the same committed bytes).
    pub fn insert(&self, key: Key, buffer: Arc<Buffer>) {
        let Some(shard) = self.shard(key.page) else {
            return;
        };
        let mut shard = shard.lock();
        if let Some(&at) = shard.map.get(&key) {
            if let Some(Some(slot)) = shard.slots.get_mut(at) {
                slot.visited = true;
            }
            return;
        }
        let at = if shard.slots.len() < self.per_shard {
            shard.slots.push(None);
            shard.slots.len().checked_sub(1)
        } else {
            Self::victim(&mut shard)
        };
        let Some(at) = at else {
            return;
        };
        if let Some(entry) = shard.slots.get_mut(at) {
            *entry = Some(Slot {
                key,
                buffer,
                visited: false,
            });
            shard.map.insert(key, at);
            drop(shard);
            self.added.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The slot CLOCK frees in a full shard: the first empty one the hand meets, or the first
    /// not read since the hand last passed, its image dropped (two turns at most).
    fn victim(shard: &mut Shard) -> Option<usize> {
        let len = shard.slots.len();
        for _ in 0..len.saturating_mul(2) {
            let at = shard.hand;
            shard.hand = match at.checked_add(1) {
                Some(next) if next < len => next,
                _ => 0,
            };
            match shard.slots.get_mut(at) {
                Some(None) => return Some(at),
                Some(Some(slot)) if slot.visited => slot.visited = false,
                Some(Some(slot)) => {
                    let key = slot.key;
                    shard.map.remove(&key);
                    if let Some(entry) = shard.slots.get_mut(at) {
                        *entry = None;
                    }
                    return Some(at);
                }
                None => return None,
            }
        }
        None
    }

    /// Drops `file`'s database file version of `page`: done before the file's page is written.
    pub fn forget_file_page(&self, file: u64, page: u64) {
        let key = Key {
            file,
            page,
            version: Version::File,
        };
        if let Some(shard) = self.shard(page) {
            let mut shard = shard.lock();
            if let Some(at) = shard.map.remove(&key) {
                if let Some(entry) = shard.slots.get_mut(at) {
                    *entry = None;
                }
            }
        }
    }

    /// Drops every database file version of `file`'s pages: done before the file is cut.
    pub fn forget_file(&self, file: u64) {
        for shard in self.shards.iter() {
            let mut shard = shard.lock();
            let files: Vec<(Key, usize)> = shard
                .map
                .iter()
                .filter(|(key, _)| key.file == file && key.version == Version::File)
                .map(|(key, at)| (*key, *at))
                .collect();
            for (key, at) in files {
                shard.map.remove(&key);
                if let Some(entry) = shard.slots.get_mut(at) {
                    *entry = None;
                }
            }
        }
    }

    /// Pages it keeps at most.
    pub fn capacity(&self) -> usize {
        self.per_shard.saturating_mul(SHARDS)
    }

    /// Images kept now.
    pub fn len(&self) -> usize {
        self.shards
            .iter()
            .map(|shard| shard.lock().map.len())
            .fold(0usize, usize::saturating_add)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Lookups and additions so far.
    pub fn counts(&self) -> Counts {
        Counts {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            added: self.added.load(Ordering::Relaxed),
        }
    }
}

/// One database file's place in a pool of shared pages: the pool, and the file's number in it.
/// Every key the file's pages take is made here, so none lacks the file's number.
#[derive(Clone)]
pub struct SharedFile {
    pages: Arc<SharedPages>,
    file: u64,
}

impl SharedFile {
    /// Joins `pages` as a file of its own.
    pub fn join(pages: Arc<SharedPages>) -> Self {
        let file = pages.join();
        Self { pages, file }
    }

    /// The pool.
    pub fn pages(&self) -> &Arc<SharedPages> {
        &self.pages
    }

    /// The key of this file's `page` at `version`.
    pub fn key(&self, page: u64, version: Version) -> Key {
        Key {
            file: self.file,
            page,
            version,
        }
    }

    /// Drops this file's database file version of `page`.
    pub fn forget_page(&self, page: u64) {
        self.pages.forget_file_page(self.file, page);
    }

    /// Drops every database file version of this file's pages.
    pub fn forget_file(&self) {
        self.pages.forget_file(self.file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(byte: u8) -> Arc<Buffer> {
        Arc::new(Buffer::new(vec![byte; 4096]))
    }

    fn file(page: u64) -> Key {
        Key {
            file: 0,
            page,
            version: Version::File,
        }
    }

    fn frame(page: u64, generation: u32, frame: u64) -> Key {
        Key {
            file: 0,
            page,
            version: Version::Frame { generation, frame },
        }
    }

    /// Do: keep a page's file image and two frame images. Expect: each found under its own
    /// version only, the same buffer as kept, and a frame of another generation not found.
    #[test]
    fn each_version_is_found_under_its_own_key() {
        let shared = SharedPages::new(1024);
        shared.insert(file(7), buffer(1));
        shared.insert(frame(7, 1, 10), buffer(2));
        shared.insert(frame(7, 1, 12), buffer(3));
        assert_eq!(shared.get(&file(7)).unwrap().as_slice()[0], 1);
        assert_eq!(shared.get(&frame(7, 1, 10)).unwrap().as_slice()[0], 2);
        assert_eq!(shared.get(&frame(7, 1, 12)).unwrap().as_slice()[0], 3);
        assert!(
            shared.get(&frame(7, 2, 10)).is_none(),
            "another generation's frame 10"
        );
        assert!(shared.get(&file(8)).is_none());
        assert_eq!(
            shared.counts(),
            Counts {
                hits: 3,
                misses: 2,
                added: 3
            }
        );
    }

    /// Do: forget a page's file version, then every file version. Expect: frame images stay.
    #[test]
    fn a_file_write_forgets_the_file_version_alone() {
        let shared = SharedPages::new(1024);
        for page in 1..=5 {
            shared.insert(file(page), buffer(1));
            shared.insert(frame(page, 1, page), buffer(2));
        }
        shared.forget_file_page(0, 3);
        assert!(shared.get(&file(3)).is_none());
        assert!(shared.get(&file(4)).is_some());
        assert!(shared.get(&frame(3, 1, 3)).is_some());
        shared.forget_file(0);
        assert!((1..=5).all(|page| shared.get(&file(page)).is_none()));
        assert!((1..=5).all(|page| shared.get(&frame(page, 1, page)).is_some()));
    }

    /// Do: add twice the bound's pages, reading a few of the first ones as they go. Expect: never
    /// more than the bound kept, and the pages read again kept over those read once.
    #[test]
    fn past_its_bound_it_evicts_by_clock() {
        let bound = SHARDS * 4;
        let shared = SharedPages::new(bound);
        let hot: Vec<u64> = (0..SHARDS as u64).collect();
        for page in 0..(bound as u64 * 2) {
            shared.insert(file(page), buffer(0));
            for &h in &hot {
                let _ = shared.get(&file(h));
            }
        }
        assert!(shared.len() <= bound, "{} kept", shared.len());
        assert!(
            hot.iter().all(|&h| shared.get(&file(h)).is_some()),
            "the pages read again stay"
        );
    }

    /// Do: two files join one pool and keep the same page at the same versions; forget one
    /// file's page, then its whole file. Expect: each finds its own image alone, and forgetting
    /// one file's versions leaves the other's (decision 20: one budget for many databases).
    #[test]
    fn files_sharing_a_pool_keep_their_pages_apart() {
        let pool = Arc::new(SharedPages::new(1024));
        let a = SharedFile::join(pool.clone());
        let b = SharedFile::join(pool.clone());
        assert_ne!(a.key(7, Version::File), b.key(7, Version::File));
        for (shared, byte) in [(&a, 1), (&b, 2)] {
            pool.insert(shared.key(7, Version::File), buffer(byte));
            let at = Version::Frame {
                generation: 1,
                frame: 3,
            };
            pool.insert(shared.key(7, at), buffer(byte + 10));
        }
        assert_eq!(pool.get(&a.key(7, Version::File)).unwrap().as_slice()[0], 1);
        assert_eq!(pool.get(&b.key(7, Version::File)).unwrap().as_slice()[0], 2);
        a.forget_page(7);
        assert!(pool.get(&a.key(7, Version::File)).is_none());
        assert!(pool.get(&b.key(7, Version::File)).is_some());
        pool.insert(a.key(8, Version::File), buffer(3));
        a.forget_file();
        assert!(pool.get(&a.key(8, Version::File)).is_none());
        assert!(pool.get(&b.key(7, Version::File)).is_some());
        let frame = Version::Frame {
            generation: 1,
            frame: 3,
        };
        assert_eq!(pool.get(&a.key(7, frame)).unwrap().as_slice()[0], 11);
        assert_eq!(pool.get(&b.key(7, frame)).unwrap().as_slice()[0], 12);
    }
}
