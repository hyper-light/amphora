//! The shell that ships a database's log and copies its bases to an object store
//! (docs/design/backup.md).
//!
//! Shipping has no timer: each [`Shipper::ship`] takes everything durable that ends at a stamped
//! commit and sends it as one segment, so a caller that ships again as soon as the last put ends
//! ships at the store's own pace, a segment holding whatever the log grew by meanwhile: its size and
//! its cadence follow the observed log rate and the store's measured put time, as the store's
//! group commit follows its writers (docs/design/store.md, decision 1).

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use amphora_api::blobs::{BlobRefusal, Blobs};

use super::object::{self, FORMAT, Identity, Manifest, Segment, StoredMark};
use super::plan::{self, BaseKey, Link, Rates, SegmentKey};
use super::{Hooks, Mark, SECOND_NS, second_of};
use crate::db::{Db, PAGE_BYTES};

/// SQLite's WAL format (sqlite.org/fileformat2.html): a 32-byte header, then frames of a 24-byte
/// header and a page.
pub(super) const LOG_HEADER_BYTES: u64 = 32;
pub(super) const FRAME_BYTES: u64 = PAGE_BYTES + 24;

/// S3's most keys a listing gives at once (ListObjectsV2, `MaxKeys`: "returns up to 1,000 key
/// names"): a listing asks for no more than a store gives.
pub(super) const LIST_PAGE: usize = 1000;

/// What a restore of 100 GB may take at most (AC-T1.12.3: 30 minutes), in seconds: the objective a
/// new base is due by ([`plan::base_due`]).
pub const RESTORE_OBJECTIVE_S: f64 = 1800.0;

/// What one round shipped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Shipped {
    pub segments: usize,
    pub bytes: u64,
}

/// A base being copied, a chunk at a time ([`Shipper::copy_base`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseCopy {
    id: u64,
    start_gen: u64,
    chunk_bytes: u64,
    file_bytes: u64,
    next: u64,
}

/// Ships one database's log, and copies its bases, under `prefix` of a store.
pub struct Shipper {
    prefix: String,
    hooks: Arc<Hooks>,
    /// The archive's count of the log's generations, and the one shipped now.
    generation: u64,
    identity: Option<Identity>,
    header: Vec<u8>,
    /// Frames of `identity` shipped.
    shipped: u64,
    pending: VecDeque<Mark>,
    through: u64,
    archived_through: u64,
    next_link: Option<Link>,
    unknown: Option<u64>,
    /// Since the latest base: whether the log broke, the bytes shipped, and that base's bytes.
    broken: bool,
    since_base: u64,
    base_bytes: Option<u64>,
    /// Marks of a generation that ended before they were shipped: their commits are in a gap.
    lost_marks: u64,
}

pub(super) fn list_all(
    blobs: &dyn Blobs,
    prefix: &str,
    until: Instant,
) -> anyhow::Result<Vec<(String, u64)>> {
    let mut all = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let page = blobs.list(prefix, after.as_deref(), LIST_PAGE, until)?;
        after = page.objects.last().map(|(k, _)| k.clone());
        all.extend(page.objects);
        if !page.truncated || after.is_none() {
            return Ok(all);
        }
        if Instant::now() >= until {
            return Err(BlobRefusal::Timeout.into());
        }
    }
}

/// Creates `key` with `bytes`; where an object is there already (an earlier create whose answer was
/// lost), it must be the same.
fn create(blobs: &dyn Blobs, key: &str, bytes: &[u8], until: Instant) -> anyhow::Result<()> {
    match blobs.create(key, bytes, until) {
        Ok(()) => Ok(()),
        Err(BlobRefusal::Exists(_)) => {
            if blobs.get(key, until)? == bytes {
                Ok(())
            } else {
                anyhow::bail!("archive object {key} exists and differs")
            }
        }
        Err(e) => Err(e.into()),
    }
}

fn identity_of(header: &[u8]) -> anyhow::Result<Identity> {
    let word = |at: usize| -> anyhow::Result<u32> {
        let bytes: [u8; 4] = header
            .get(at..at.saturating_add(4))
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| anyhow::anyhow!("the log's header is short"))?;
        Ok(u32::from_be_bytes(bytes))
    };
    if u64::from(word(8)?) != PAGE_BYTES {
        anyhow::bail!("the log's pages aren't {PAGE_BYTES} bytes");
    }
    Ok(Identity {
        seq: word(12)?,
        salt1: word(16)?,
        salt2: word(20)?,
    })
}

pub(super) fn frame_offset(frame: u64) -> anyhow::Result<u64> {
    frame
        .checked_mul(FRAME_BYTES)
        .and_then(|b| b.checked_add(LOG_HEADER_BYTES))
        .ok_or_else(|| anyhow::anyhow!("a frame's offset overflows"))
}

impl Shipper {
    /// A shipper for `db`'s log under `prefix`, carrying on from what the archive holds: the
    /// generation it last shipped, if the log is still in it, else a new generation, the log broken.
    /// Commits the log holds now that weren't stamped are stamped now, their seconds since the
    /// archive's last unknown.
    pub fn open(
        prefix: &str,
        hooks: Arc<Hooks>,
        blobs: &dyn Blobs,
        db: &Db,
        until: Instant,
    ) -> anyhow::Result<Self> {
        let mut shipper = Self {
            prefix: prefix.into(),
            hooks,
            generation: 0,
            identity: None,
            header: Vec::new(),
            shipped: 0,
            pending: VecDeque::new(),
            through: 0,
            archived_through: 0,
            next_link: None,
            unknown: None,
            broken: true,
            since_base: 0,
            base_bytes: None,
            lost_marks: 0,
        };
        let segments = list_all(blobs, &format!("{prefix}/log/"), until)?;
        if let Some((key, _)) = segments.last() {
            let last = SegmentKey::parse(prefix, key)?;
            let body: Segment = object::decode(key, &blobs.get(key, until)?)?;
            shipper.generation = last.generation;
            shipper.archived_through = last.through;
            shipper.unknown = Some(last.through);
            if let Some(head) = db.read_log(0, usize::try_from(LOG_HEADER_BYTES)?)?
                && identity_of(&head)? == body.identity
            {
                shipper.identity = Some(body.identity);
                shipper.header = head;
                shipper.shipped = last.last;
            }
        }
        // The latest base and what was shipped after it.
        let bases = list_all(blobs, &format!("{prefix}/base/"), until)?;
        let latest = bases
            .iter()
            .filter_map(|(k, _)| BaseKey::parse(prefix, k).ok().map(|b| (b, k)))
            .max_by_key(|(b, _)| (b.end, b.id));
        if let Some((base, key)) = latest {
            let manifest: Manifest = object::decode(key, &blobs.get(key, until)?)?;
            shipper.base_bytes = Some(manifest.file_bytes);
            shipper.broken = false;
            for (key, bytes) in &segments {
                let s = SegmentKey::parse(prefix, key)?;
                if (s.generation, s.last) > base.end {
                    shipper.since_base = shipper.since_base.saturating_add(*bytes);
                }
                if s.link == Some(Link::Gap) && s.generation > base.end.0 {
                    shipper.broken = true;
                }
            }
        }
        let (seq, end) = db.log_end();
        if end > shipper.shipped {
            shipper.hooks.marks.committed(seq, end)?;
        }
        Ok(shipper)
    }

    /// Ships what is durable and stamped: one segment, split into parts where it outgrows the
    /// store's objects. Nothing when nothing is.
    pub fn ship(&mut self, db: &Db, blobs: &dyn Blobs, until: Instant) -> anyhow::Result<Shipped> {
        let drained = self.hooks.marks.drain()?;
        self.pending.extend(drained.marks);
        self.through = drained.through;
        // What was committed before now is durable once this ends (store.md, decision 2): every
        // mark drained is then of a durable commit.
        (db.flusher())()?;
        let (seq, end) = db.log_end();
        let durable = db.durable_frame().unwrap_or(0).min(end);
        let Some(head) = db.read_log(0, usize::try_from(LOG_HEADER_BYTES)?)? else {
            return Ok(Shipped::default());
        };
        let identity = identity_of(&head)?;
        if identity.seq != seq {
            // The log was started again and its header isn't written yet.
            return Ok(Shipped::default());
        }
        if self.identity != Some(identity) {
            self.begin_generation(identity, head)?;
        }
        // Marks of a generation shipped no further are of commits in a gap.
        while let Some(m) = self.pending.front() {
            if m.seq == identity.seq && m.frame > self.shipped {
                break;
            }
            if m.seq != identity.seq {
                self.lost_marks = self.lost_marks.saturating_add(1);
            }
            self.pending.pop_front();
        }
        let ready = self
            .pending
            .iter()
            .take_while(|m| m.seq == identity.seq && m.frame <= durable)
            .count();
        if ready == 0 {
            return Ok(Shipped::default());
        }
        let mut marks: Vec<Mark> = self.pending.drain(..ready).collect();
        if let (Some(first), Some(unknown)) = (marks.first_mut(), self.unknown) {
            first.unknown_from = Some(first.unknown_from.map_or(unknown, |u| u.min(unknown)));
        }
        let last = marks.last().map_or(self.shipped, |m| m.frame);
        // Every commit stamped at or before `through` is in this segment or before it: those after
        // it are in the next mark's second, or weren't drained yet.
        let through = match self.pending.front() {
            Some(next) => {
                let second_before = second_of(next.ns)
                    .saturating_sub(1)
                    .saturating_mul(SECOND_NS);
                next.unknown_from
                    .map_or(second_before, |u| u.min(second_before))
            }
            None => self.through,
        };
        let shipped = self.send(db, blobs, &marks, last, through, until)?;
        self.shipped = last;
        self.hooks.fence.shipped(identity.seq, last)?;
        self.archived_through = through;
        self.unknown = None;
        self.next_link = None;
        self.since_base = self.since_base.saturating_add(shipped.bytes);
        Ok(shipped)
    }

    fn begin_generation(&mut self, identity: Identity, head: Vec<u8>) -> anyhow::Result<()> {
        let link = match self.identity {
            Some(old) if self.hooks.fence.ended_whole(old.seq, self.shipped)? => {
                Link::Whole(self.shipped)
            }
            _ => Link::Gap,
        };
        if link == Link::Gap {
            self.broken = true;
            if self.generation > 0 {
                self.unknown = Some(
                    self.unknown
                        .map_or(self.archived_through, |u| u.min(self.archived_through)),
                );
            }
        }
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("the archive's generations overflow"))?;
        self.identity = Some(identity);
        self.header = head;
        self.shipped = 0;
        self.next_link = Some(link);
        Ok(())
    }

    /// Sends frames `shipped + 1..=last` with `marks`: in as few objects as the store takes.
    fn send(
        &self,
        db: &Db,
        blobs: &dyn Blobs,
        marks: &[Mark],
        last: u64,
        through: u64,
        until: Instant,
    ) -> anyhow::Result<Shipped> {
        let identity = self
            .identity
            .ok_or_else(|| anyhow::anyhow!("no log generation to ship"))?;
        let marks_stored: Vec<StoredMark> = marks
            .iter()
            .map(|m| StoredMark {
                frame: m.frame,
                ns: m.ns,
                unknown_from: m.unknown_from,
            })
            .collect();
        let mut body = Segment {
            format: FORMAT,
            generation: self.generation,
            identity,
            first: self.shipped.saturating_add(1),
            header: self.header.clone(),
            marks: marks_stored,
            frames: Vec::new(),
        };
        let room = blobs
            .object_max()
            .checked_sub(object::segment_overhead(&body)?)
            .ok_or_else(|| anyhow::anyhow!("the store's objects can't hold a segment's marks"))?;
        let per_object = u64::try_from(room)?.checked_div(FRAME_BYTES).unwrap_or(0);
        if per_object == 0 {
            anyhow::bail!("the store's objects can't hold one frame of the log");
        }
        let mut stored = std::mem::take(&mut body.marks);
        let mut shipped = Shipped::default();
        let mut from = self.shipped;
        while from < last {
            let to = from.saturating_add(per_object).min(last);
            let is_last = to == last;
            body.first = from.saturating_add(1);
            body.frames = db
                .read_log(
                    frame_offset(from)?,
                    usize::try_from(to.saturating_sub(from).saturating_mul(FRAME_BYTES))?,
                )?
                .ok_or_else(|| anyhow::anyhow!("the log's file is gone"))?;
            let key = SegmentKey {
                generation: self.generation,
                first: body.first,
                last: to,
                marks: if is_last {
                    marks.first().zip(marks.last()).map(|(a, b)| (a.ns, b.ns))
                } else {
                    None
                },
                through,
                unknown: if is_last {
                    marks.first().and_then(|m| m.unknown_from)
                } else {
                    None
                },
                link: if body.first == 1 {
                    self.next_link
                } else {
                    None
                },
            };
            // The marks go in the last part only: the others end at no mark.
            if is_last {
                body.marks = std::mem::take(&mut stored);
            }
            let bytes = object::encode(&body)?;
            create(blobs, &key.key(&self.prefix), &bytes, until)?;
            shipped.segments = shipped.segments.saturating_add(1);
            shipped.bytes = shipped.bytes.saturating_add(u64::try_from(bytes.len())?);
            from = to;
        }
        Ok(shipped)
    }

    /// Whether a new base is due ([`plan::base_due`]), at the restore `rates` measured, if any.
    #[must_use]
    pub fn base_due(&self, rates: Option<Rates>) -> bool {
        plan::base_due(
            self.base_bytes,
            self.broken,
            self.since_base,
            rates,
            RESTORE_OBJECTIVE_S,
        )
    }

    /// Marks dropped because their generation ended unshipped.
    #[must_use]
    pub fn lost_marks(&self) -> u64 {
        self.lost_marks
    }

    /// Begins a copy of the database's file, in chunks as large as the store's objects and
    /// `memory` allow: a chunk is a request, and a restore's fetch moves more bytes a second the
    /// fewer requests it waits on. Refused until the log's generation now has been seen by
    /// [`Shipper::ship`].
    pub fn begin_base(
        &mut self,
        db: &Db,
        blobs: &dyn Blobs,
        memory: usize,
    ) -> anyhow::Result<BaseCopy> {
        let (seq, _) = db.log_end();
        if self.identity.is_none_or(|i| i.seq != seq) {
            anyhow::bail!("ship the log's generation before copying a base in it");
        }
        let room = blobs.object_max().saturating_sub(32).min(memory);
        let chunk_bytes = u64::try_from(room)?
            .checked_div(PAGE_BYTES)
            .unwrap_or(0)
            .saturating_mul(PAGE_BYTES);
        if chunk_bytes == 0 {
            anyhow::bail!("a base's chunk can't hold a page");
        }
        let id = {
            let mut state = self.hooks.marks.state()?;
            self.hooks.marks.stamp(&mut state)
        };
        Ok(BaseCopy {
            id,
            start_gen: self.generation,
            chunk_bytes,
            file_bytes: db.file_bytes()?,
            next: 0,
        })
    }

    /// Copies the next chunk: whether more follow.
    pub fn copy_base(
        &mut self,
        db: &Db,
        blobs: &dyn Blobs,
        copy: &mut BaseCopy,
        until: Instant,
    ) -> anyhow::Result<bool> {
        let from = copy.next.saturating_mul(copy.chunk_bytes);
        if from >= copy.file_bytes {
            return Ok(false);
        }
        let len = copy.chunk_bytes.min(copy.file_bytes.saturating_sub(from));
        let bytes = object::sealed(&db.read_file(from, usize::try_from(len)?)?)?;
        create(
            blobs,
            &plan::chunk_key(&self.prefix, copy.id, copy.next),
            &bytes,
            until,
        )?;
        copy.next = copy.next.saturating_add(1);
        Ok(copy.next.saturating_mul(copy.chunk_bytes) < copy.file_bytes)
    }

    /// Ends a base whose chunks are all copied: its manifest names where the log stood once the
    /// copy ended, which a restore replays through at least.
    pub fn end_base(
        &mut self,
        db: &Db,
        blobs: &dyn Blobs,
        copy: BaseCopy,
        until: Instant,
    ) -> anyhow::Result<()> {
        if copy.next.saturating_mul(copy.chunk_bytes) < copy.file_bytes {
            anyhow::bail!("the base's chunks aren't all copied");
        }
        self.ship(db, blobs, until)?;
        let (seq, end) = db.log_end();
        if self.identity.is_none_or(|i| i.seq != seq) {
            anyhow::bail!("ship the log's generation before ending a base in it");
        }
        let manifest = Manifest {
            format: FORMAT,
            id: copy.id,
            start_gen: copy.start_gen,
            end: (self.generation, end),
            file_bytes: copy.file_bytes,
            chunk_bytes: copy.chunk_bytes,
            chunks: copy.next,
        };
        let key = BaseKey {
            end: manifest.end,
            start_gen: manifest.start_gen,
            id: manifest.id,
        };
        create(
            blobs,
            &key.key(&self.prefix),
            &object::encode(&manifest)?,
            until,
        )?;
        self.base_bytes = Some(copy.file_bytes);
        self.since_base = 0;
        self.broken = false;
        Ok(())
    }
}
