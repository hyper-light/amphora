//! Amphora: the packed log (Amphora's `docs/design/store.md`, decision 16).
//!
//! A generation of the log whose header carries [`PACKED_FORMAT`] holds records instead of one
//! page a frame. A prepared batch of pages (a commit's, or a spill's) is cut into records of up to
//! [`pages_per_record`] pages, each the pages' images compressed together (LZ4) and laid in as many
//! of the log's fixed-size frame slots as it takes. Every page of a record maps to the record's
//! first slot, so frame ids keep their meaning everywhere else (read marks, backfill, restarts).
//! The first slot's frame header carries the record's slot count where a page number would be,
//! the others [`CONTINUATION`]; the record's last slot carries the commit's database size when it
//! ends one. The slots' checksums chain as frames' do.

use std::collections::VecDeque;
use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::error::LimboError;
use crate::storage::sqlite3_ondisk::WAL_FRAME_HEADER_SIZE;
use crate::Result;

/// The log header's file format for a generation of one page a frame (SQLite's).
pub(crate) const PAGE_FORMAT: u32 = 3_007_000;

/// The log header's file format for a packed generation.
pub(crate) const PACKED_FORMAT: u32 = 0x414d_5001;

/// The page-number field of a record's slots after its first.
pub(crate) const CONTINUATION: u32 = u32::MAX;

/// A record's payload starts with its stored and raw lengths.
const PAYLOAD_HEADER: usize = 8;

/// Pages a record holds at most: a prepared batch is at most 1024 (`IOV_MAX` in `wal.rs`).
const MOST_PAGES: usize = 1024;

/// Raw bytes a record is cut at: LZ4 finds a match at most 64 KiB back (its block format,
/// `lz4_Block_format.md`), so a larger record compresses little better, while a reader that misses
/// its page cache reads and decodes a whole record. At 4 KiB pages that is 64 pages, about 18 slots
/// at the 3x measured for whole pages (decision 16), the last slot's padding about 3% of it.
const RECORD_BYTES: usize = 256 << 10;

/// Pages a record of `page_size` pages holds at most: as many as fill [`RECORD_BYTES`], 1 to
/// [`MOST_PAGES`].
pub(crate) fn pages_per_record(page_size: usize) -> usize {
    RECORD_BYTES
        .checked_div(page_size)
        .unwrap_or(1)
        .clamp(1, MOST_PAGES)
}

fn corrupt(what: &str) -> LimboError {
    LimboError::Corrupt(format!("packed log record: {what}"))
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32> {
    let end = at
        .checked_add(4)
        .ok_or_else(|| corrupt("offset overflows"))?;
    let word: [u8; 4] = bytes
        .get(at..end)
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| corrupt("cut short"))?;
    Ok(u32::from_be_bytes(word))
}

fn as_usize(n: u32) -> Result<usize> {
    usize::try_from(n).map_err(|_| corrupt("length too large"))
}

/// The raw bytes of `pages`: [page count][page numbers][page images], u32s big-endian.
fn raw_len(count: usize, page_size: usize) -> Option<usize> {
    let per_page = page_size.checked_add(4)?;
    count.checked_mul(per_page)?.checked_add(4)
}

/// A record's payload for `pages` (each image `page_size` bytes, each page once): [stored
/// length][raw length][the raw bytes compressed], or, where LZ4 doesn't make them smaller, stored
/// length 0 and the raw bytes as they are (an LZ4 block of anything is never empty).
pub(crate) fn encode(pages: &[(u32, &[u8])], page_size: usize) -> Result<Vec<u8>> {
    if pages.is_empty() || pages.len() > MOST_PAGES {
        return Err(corrupt("a record holds 1 to 1024 pages"));
    }
    let count = u32::try_from(pages.len()).map_err(|_| corrupt("too many pages"))?;
    let len = raw_len(pages.len(), page_size).ok_or_else(|| corrupt("record too large"))?;
    let mut raw = Vec::with_capacity(len);
    raw.extend_from_slice(&count.to_be_bytes());
    for (page, _) in pages {
        raw.extend_from_slice(&page.to_be_bytes());
    }
    for (_, image) in pages {
        if image.len() != page_size {
            return Err(corrupt("an image isn't a page long"));
        }
        raw.extend_from_slice(image);
    }
    let compressed = lz4_flex::block::compress(&raw);
    let (stored_len, stored) = if compressed.len() < raw.len() {
        let n = u32::try_from(compressed.len()).map_err(|_| corrupt("record too large"))?;
        (n, compressed.as_slice())
    } else {
        (0, raw.as_slice())
    };
    let raw_len = u32::try_from(raw.len()).map_err(|_| corrupt("record too large"))?;
    let mut payload = Vec::with_capacity(PAYLOAD_HEADER.saturating_add(stored.len()));
    payload.extend_from_slice(&stored_len.to_be_bytes());
    payload.extend_from_slice(&raw_len.to_be_bytes());
    payload.extend_from_slice(stored);
    Ok(payload)
}

/// The most slots a record of `page_size` pages takes, whatever wrote it: a payload's header and
/// the raw bytes of [`MOST_PAGES`] pages, stored or compressed (LZ4's bound for bytes that don't
/// compress). A first slot that claims more is no record's.
pub(crate) fn most_slots(page_size: usize) -> Result<u32> {
    let raw = raw_len_bound(page_size)?;
    let stored = lz4_flex::block::get_maximum_output_size(raw).max(raw);
    let payload = PAYLOAD_HEADER
        .checked_add(stored)
        .ok_or_else(|| corrupt("record too large"))?;
    slots(payload, page_size)
}

/// Fills the page body of a record's `slot`-th slot (0-based) with its share of `payload`, the
/// rest zeros.
pub(crate) fn fill_slot(payload: &[u8], slot: usize, body: &mut [u8]) -> Result<()> {
    let start = slot
        .checked_mul(body.len())
        .ok_or_else(|| corrupt("slot offset overflows"))?;
    let share = payload.get(start..).unwrap_or_default();
    let n = share.len().min(body.len());
    let (filled, rest) = body
        .split_at_mut_checked(n)
        .ok_or_else(|| corrupt("slot cut short"))?;
    filled.copy_from_slice(share.get(..n).ok_or_else(|| corrupt("payload cut short"))?);
    rest.fill(0);
    Ok(())
}

/// A record's payload from its slots as read from the log, each a frame header and a page body.
pub(crate) fn gather(slots: &[u8], page_size: usize) -> Result<Vec<u8>> {
    let frame = WAL_FRAME_HEADER_SIZE
        .checked_add(page_size)
        .ok_or_else(|| corrupt("frame size overflows"))?;
    if page_size == 0 || slots.len().checked_rem(frame) != Some(0) {
        return Err(corrupt("slots aren't whole frames"));
    }
    let mut payload = Vec::with_capacity(slots.len());
    for slot in slots.chunks_exact(frame) {
        payload.extend_from_slice(
            slot.get(WAL_FRAME_HEADER_SIZE..)
                .ok_or_else(|| corrupt("slot cut short"))?,
        );
    }
    Ok(payload)
}

/// The slots a payload of `payload_len` bytes takes, a slot holding `page_size` bytes of it.
pub(crate) fn slots(payload_len: usize, page_size: usize) -> Result<u32> {
    if page_size == 0 {
        return Err(corrupt("page size 0"));
    }
    let slots = payload_len.div_ceil(page_size).max(1);
    u32::try_from(slots).map_err(|_| corrupt("record takes too many slots"))
}

/// The slot count a record's first slot carries in its frame header's page-number field: one that
/// is neither 0 (no frame) nor [`CONTINUATION`].
pub(crate) fn is_first_slot(page_field: u32) -> bool {
    page_field != 0 && page_field != CONTINUATION
}

/// A record, decoded: the pages it holds and their images.
#[derive(Debug)]
pub(crate) struct Record {
    pages: Vec<u32>,
    raw: Vec<u8>,
    images_at: usize,
    page_size: usize,
}

impl Record {
    /// Decodes a record from its slots' bytes (`payload`, padding after it ignored).
    pub(crate) fn decode(payload: &[u8], page_size: usize) -> Result<Self> {
        let stored_len = as_usize(u32_at(payload, 0)?)?;
        let raw_len = as_usize(u32_at(payload, 4)?)?;
        let most = raw_len_bound(page_size)?;
        if raw_len > most {
            return Err(corrupt("raw length beyond any record's"));
        }
        let end = PAYLOAD_HEADER
            .checked_add(if stored_len == 0 { raw_len } else { stored_len })
            .ok_or_else(|| corrupt("stored length overflows"))?;
        let stored = payload
            .get(PAYLOAD_HEADER..end)
            .ok_or_else(|| corrupt("stored bytes cut short"))?;
        let raw = if stored_len == 0 {
            stored.to_vec()
        } else {
            let mut raw = vec![0u8; raw_len];
            let n = lz4_flex::block::decompress_into(stored, &mut raw)
                .map_err(|e| corrupt(&format!("doesn't decompress: {e}")))?;
            if n != raw_len {
                return Err(corrupt("decompressed length isn't the raw length"));
            }
            raw
        };
        let count = as_usize(u32_at(&raw, 0)?)?;
        if count == 0 || count > MOST_PAGES {
            return Err(corrupt("page count out of range"));
        }
        if raw_len_of(count, page_size)? != raw_len {
            return Err(corrupt("raw length isn't its pages'"));
        }
        let mut pages = Vec::with_capacity(count);
        for i in 0..count {
            let at = i
                .checked_mul(4)
                .and_then(|o| o.checked_add(4))
                .ok_or_else(|| corrupt("page number offset overflows"))?;
            pages.push(u32_at(&raw, at)?);
        }
        let images_at = count
            .checked_mul(4)
            .and_then(|o| o.checked_add(4))
            .ok_or_else(|| corrupt("image offset overflows"))?;
        Ok(Self {
            pages,
            raw,
            images_at,
            page_size,
        })
    }

    /// The pages the record holds, in its order.
    pub(crate) fn pages(&self) -> &[u32] {
        &self.pages
    }

    /// The image of `page`, if the record holds it.
    pub(crate) fn image(&self, page: u32) -> Option<&[u8]> {
        let index = self.pages.iter().position(|p| *p == page)?;
        let start = index
            .checked_mul(self.page_size)?
            .checked_add(self.images_at)?;
        let end = start.checked_add(self.page_size)?;
        self.raw.get(start..end)
    }

    /// The bytes it holds once decoded, for a cache's budget.
    pub(crate) fn size(&self) -> usize {
        self.raw.len()
    }
}

/// Bytes the decoded records may hold. A record is decoded for a reader's or the checkpoint's miss,
/// and a burst of misses mostly falls in a few records: the oldest goes first past this.
pub(crate) const DECODED_BUDGET: usize = 16 << 20;

/// What the shared log keeps of its packed records: each record's slot count by its first slot
/// (written before any page maps to the record), and records decoded, keyed by the log's
/// generation (its header's checkpoint sequence) and first slot, within [`DECODED_BUDGET`].
#[derive(Debug, Default)]
pub(crate) struct Records {
    slots: FxHashMap<u64, u32>,
    decoded: FxHashMap<(u32, u64), Arc<Record>>,
    order: VecDeque<(u32, u64)>,
    bytes: usize,
}

impl Records {
    /// A record of `slots` slots now begins at `head`: whatever was decoded there before is dropped
    /// (a rollback or a restart can lay a new record in the same slots).
    pub(crate) fn publish(&mut self, head: u64, slots: u32) {
        self.slots.insert(head, slots);
        self.forget(|(_, h)| h == head);
    }

    /// The slots of the record at `head`.
    pub(crate) fn slots_of(&self, head: u64) -> Option<u32> {
        self.slots.get(&head).copied()
    }

    /// The first slot of the record whose slots hold `frame`, if one does.
    pub(crate) fn covering(&self, frame: u64) -> Option<u64> {
        self.slots.iter().find_map(|(head, slots)| {
            let last = head.checked_add(u64::from(*slots))?.checked_sub(1)?;
            if *head <= frame && frame <= last {
                Some(*head)
            } else {
                None
            }
        })
    }

    /// The record at `head` of `generation`, if it was decoded and kept.
    pub(crate) fn decoded(&self, generation: u32, head: u64) -> Option<Arc<Record>> {
        self.decoded.get(&(generation, head)).cloned()
    }

    /// Keeps a decoded record, the oldest kept going first past the budget.
    pub(crate) fn keep(&mut self, generation: u32, head: u64, record: Arc<Record>) {
        let size = record.size();
        if size > DECODED_BUDGET || self.decoded.contains_key(&(generation, head)) {
            return;
        }
        while self.bytes.saturating_add(size) > DECODED_BUDGET {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(gone) = self.decoded.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(gone.size());
            }
        }
        self.decoded.insert((generation, head), record);
        self.order.push_back((generation, head));
        self.bytes = self.bytes.saturating_add(size);
    }

    /// Forgets the records past `max_frame` (a rollback's).
    pub(crate) fn drop_after(&mut self, max_frame: u64) {
        self.slots.retain(|head, _| *head <= max_frame);
        self.forget(|(_, h)| h > max_frame);
    }

    /// Forgets every record (a restart's or a truncation's).
    pub(crate) fn clear(&mut self) {
        self.slots.clear();
        self.decoded.clear();
        self.order.clear();
        self.bytes = 0;
    }

    fn forget(&mut self, gone: impl Fn((u32, u64)) -> bool) {
        let mut freed = 0usize;
        self.decoded.retain(|key, record| {
            if gone(*key) {
                freed = freed.saturating_add(record.size());
                false
            } else {
                true
            }
        });
        self.order.retain(|key| !gone(*key));
        self.bytes = self.bytes.saturating_sub(freed);
    }
}

fn raw_len_of(count: usize, page_size: usize) -> Result<usize> {
    raw_len(count, page_size).ok_or_else(|| corrupt("record too large"))
}

fn raw_len_bound(page_size: usize) -> Result<usize> {
    raw_len_of(MOST_PAGES, page_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(fill: u8, size: usize) -> Vec<u8> {
        (0..size)
            .map(|i| fill.wrapping_add((i % 7) as u8))
            .collect()
    }

    #[test]
    fn a_record_decodes_to_the_pages_it_was_made_of() {
        let size = 4096;
        let (a, b, c) = (page(1, size), page(2, size), vec![0u8; size]);
        let payload = encode(&[(7, &a), (3, &b), (900, &c)], size).unwrap();
        assert!(payload.len() < 3 * size, "the record didn't compress");
        let record = Record::decode(&payload, size).unwrap();
        assert_eq!(record.pages(), &[7, 3, 900]);
        assert_eq!(record.image(7).unwrap(), &a[..]);
        assert_eq!(record.image(3).unwrap(), &b[..]);
        assert_eq!(record.image(900).unwrap(), &c[..]);
        assert!(record.image(8).is_none());
    }

    #[test]
    fn padding_after_a_payload_is_ignored() {
        let size = 1024;
        let a = page(9, size);
        let mut payload = encode(&[(1, &a)], size).unwrap();
        let n = slots(payload.len(), size).unwrap() as usize;
        payload.resize(n * size, 0);
        let record = Record::decode(&payload, size).unwrap();
        assert_eq!(record.image(1).unwrap(), &a[..]);
    }

    #[test]
    fn a_payload_takes_the_slots_it_fills() {
        assert_eq!(slots(1, 4096).unwrap(), 1);
        assert_eq!(slots(4096, 4096).unwrap(), 1);
        assert_eq!(slots(4097, 4096).unwrap(), 2);
        assert_eq!(slots(0, 4096).unwrap(), 1);
        assert!(slots(10, 0).is_err());
    }

    #[test]
    fn a_damaged_record_is_refused_not_read() {
        let size = 512;
        let a = page(4, size);
        let payload = encode(&[(5, &a), (6, &a)], size).unwrap();
        // Cut short anywhere, or its lengths or bytes damaged: an error, never a panic.
        for cut in 0..payload.len() {
            assert!(
                Record::decode(&payload[..cut], size).is_err(),
                "cut at {cut}"
            );
        }
        for at in 0..payload.len() {
            let mut damaged = payload.clone();
            damaged[at] ^= 0xa5;
            if let Ok(record) = Record::decode(&damaged, size) {
                // A flipped byte in a page image can decode; its pages stay well formed.
                for p in record.pages() {
                    assert_eq!(record.image(*p).map(<[u8]>::len), Some(size));
                }
            }
        }
        let mut huge = payload;
        huge[4..8].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(Record::decode(&huge, size).is_err());
    }

    #[test]
    fn a_record_holds_one_to_1024_pages_each_a_page_long() {
        let size = 512;
        let a = page(1, size);
        assert!(encode(&[], size).is_err());
        assert!(encode(&[(1, &a[..100])], size).is_err());
        let many: Vec<(u32, &[u8])> = (1..=1025u32).map(|p| (p, &a[..])).collect();
        assert!(encode(&many, size).is_err());
        assert!(encode(&many[..1024], size).is_ok());
    }

    #[test]
    fn decoded_records_stay_within_their_budget_and_go_with_their_slots() {
        let size = 4096;
        let a = page(1, size);
        let images: Vec<(u32, &[u8])> = (1..=512u32).map(|p| (p, &a[..])).collect();
        let big = Arc::new(Record::decode(&encode(&images, size).unwrap(), size).unwrap());
        let mut records = Records::default();
        for head in 1..=40u64 {
            records.publish(head * 10, 3);
            records.keep(1, head * 10, big.clone());
            assert!(records.bytes <= DECODED_BUDGET);
        }
        assert!(records.decoded(1, 400).is_some(), "the newest is kept");
        assert!(records.decoded(1, 10).is_none(), "the oldest went first");
        assert_eq!(records.slots_of(10), Some(3));
        // Republished slots drop what was decoded there; a rollback forgets what's past its end.
        records.publish(400, 2);
        assert!(records.decoded(1, 400).is_none());
        assert_eq!(records.slots_of(400), Some(2));
        records.drop_after(300);
        assert_eq!(records.slots_of(390), None);
        assert_eq!(records.slots_of(300), Some(3));
        assert!(records.decoded(1, 390).is_none());
        records.clear();
        assert_eq!(records.slots_of(10), None);
        assert_eq!(records.bytes, 0);
    }

    #[test]
    fn what_lz4_doesnt_shrink_is_stored_as_it_is() {
        let size = 512;
        // Bytes with no repeats LZ4 could use: a xorshift's.
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let noise: Vec<u8> = (0..size)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect();
        let payload = encode(&[(2, &noise)], size).unwrap();
        assert_eq!(&payload[..4], &[0, 0, 0, 0], "stored, not compressed");
        let record = Record::decode(&payload, size).unwrap();
        assert_eq!(record.image(2).unwrap(), &noise[..]);
    }

    #[test]
    fn slots_filled_from_a_payload_gather_back_to_it() {
        let size = 1024;
        let a = page(3, size);
        let images: Vec<(u32, &[u8])> = (1..=9u32).map(|p| (p, &a[..])).collect();
        let payload = encode(&images, size).unwrap();
        let n = slots(payload.len(), size).unwrap() as usize;
        let frame = WAL_FRAME_HEADER_SIZE + size;
        let mut log = vec![0xffu8; n * frame];
        for (i, slot) in log.chunks_exact_mut(frame).enumerate() {
            fill_slot(&payload, i, &mut slot[WAL_FRAME_HEADER_SIZE..]).unwrap();
        }
        let gathered = gather(&log, size).unwrap();
        assert_eq!(&gathered[..payload.len()], &payload[..]);
        assert!(
            gathered[payload.len()..].iter().all(|b| *b == 0),
            "padding is zeros"
        );
        let record = Record::decode(&gathered, size).unwrap();
        assert_eq!(record.pages(), (1..=9u32).collect::<Vec<_>>().as_slice());
        assert!(gather(&log[..frame - 1], size).is_err(), "a slot cut short");
    }

    #[test]
    fn no_record_takes_more_slots_than_most_slots_says() {
        for size in [512usize, 4096, 65536] {
            assert!(pages_per_record(size) >= 1 && pages_per_record(size) <= MOST_PAGES);
            let most = most_slots(size).unwrap() as usize;
            assert!(
                most >= MOST_PAGES,
                "1024 pages that don't compress take a slot each at least"
            );
        }
        assert_eq!(pages_per_record(4096), 64);
        assert_eq!(pages_per_record(65536), 4);
        assert_eq!(pages_per_record(512), 512);
    }

    #[test]
    fn a_frame_is_found_in_the_record_that_covers_it() {
        let mut records = Records::default();
        records.publish(5, 3);
        records.publish(8, 1);
        assert_eq!(records.covering(4), None);
        assert_eq!(records.covering(5), Some(5));
        assert_eq!(records.covering(7), Some(5));
        assert_eq!(records.covering(8), Some(8));
        assert_eq!(records.covering(9), None);
    }

    #[test]
    fn a_first_slot_is_told_from_a_continuation() {
        assert!(is_first_slot(1));
        assert!(is_first_slot(513));
        assert!(!is_first_slot(0));
        assert!(!is_first_slot(CONTINUATION));
    }
}
