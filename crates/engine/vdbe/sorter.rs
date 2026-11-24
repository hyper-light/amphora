use crate::types::IOResultOr;
use crate::{turso_assert, turso_assert_eq};
use amphora_engine_parser::ast::SortOrder;
use branches::mark_unlikely;

use crate::sync::RwLock;
use crate::sync::{atomic, Arc};
use bumpalo::Bump;
use std::cmp::{Eq, Ord, Ordering, PartialEq, PartialOrd, Reverse};
use std::collections::VecDeque;
use std::ptr::NonNull;
use std::rc::Rc;

use crate::alloc::vec;
use crate::alloc::*;
use crate::error::LimboError;
use crate::io::TempFile;
use crate::io::{Buffer, Completion, CompletionGroup, File, IO};
use crate::storage::sqlite3_ondisk::{read_varint, varint_len, write_varint};
use crate::translate::collate::CollationSeq;
use crate::types::{cmp_in_column, cmp_with_sort, IOCompletions, ValueIterator};
use crate::types::{IOResult, ImmutableRecord, KeyInfo, RecordBuf, Value, ValueRef};
use crate::Result;
use crate::{io_yield_one, return_if_io, CompletionError};

/// A custom comparison function for sorting custom type columns.
/// Takes two value references and returns an Ordering.
/// Used when a custom type defines a `<` operator for correct sort behavior.
pub type SortComparator = Arc<dyn Fn(&ValueRef, &ValueRef) -> Result<Ordering> + Send + Sync>;

/// Bit position of the 3-bit class rank in a normalized key.
const NORM_CLASS_SHIFT: u32 = 61;

/// Order-preserving 64-bit prefix of the first sort-key column.
///
/// Layout: 3-bit class rank | 61-bit payload. Class ranks follow the SQL type
/// ordering (NULL < numeric < text < blob), with NULL remapped above blob when
/// the effective NULLS placement requires it. The whole key is bit-inverted for
/// DESC so a plain `u64` comparison applies the sort direction.
///
/// Invariant: `norm < other_norm` implies the full key comparison orders this
/// record first, so the sort comparator only falls back to the full (collation
/// and comparator aware) comparison when two normalized keys are equal. The
/// returned `decisive` flag is true when equal normalized keys additionally
/// prove the full keys are equal, letting the comparator skip the fallback.
fn normalized_first_key(
    values: &[ValueRef<'_>],
    key_info: &[KeyInfo],
    comparators: &[Option<SortComparator>],
) -> (u64, bool) {
    use crate::numeric::Numeric;
    let (Some(value), Some(key)) = (values.first(), key_info.first()) else {
        return (0, false);
    };
    if comparators.first().is_some_and(|c| c.is_some()) {
        // Custom ordering: the normalized key cannot mirror it.
        return (0, false);
    }
    let (norm, mut decisive) = match value {
        ValueRef::Null => {
            // Rank NULL above blobs when it must sort after non-NULL values in
            // the pre-inversion key space. With `nulls_order` unset the natural
            // rank is correct for ASC, and the DESC bit inversion below moves
            // NULL last, matching the reversed comparison.
            let rank_high = match (key.nulls_order, key.sort_order) {
                (None, _) => false,
                (Some(amphora_engine_parser::ast::NullsOrder::First), SortOrder::Asc) => false,
                (Some(amphora_engine_parser::ast::NullsOrder::First), SortOrder::Desc) => true,
                (Some(amphora_engine_parser::ast::NullsOrder::Last), SortOrder::Asc) => true,
                (Some(amphora_engine_parser::ast::NullsOrder::Last), SortOrder::Desc) => false,
            };
            (
                if rank_high {
                    4u64 << NORM_CLASS_SHIFT
                } else {
                    0
                },
                true,
            )
        }
        ValueRef::Numeric(n) => {
            // Map both integers and floats through the monotone f64 encoding so
            // cross-type comparisons stay ordered (rounding to nearest is
            // monotone). Equal-after-truncation pairs fall back to the exact
            // comparison; `decisive` only covers integers whose f64 form is
            // exact and injective after dropping the low 3 mantissa bits.
            let (f, exact) = match n {
                Numeric::Integer(i) => (*i as f64, i.unsigned_abs() < (1u64 << 49)),
                Numeric::Float(f) => (f64::from(*f), false),
            };
            let f = if f == 0.0 { 0.0 } else { f }; // collapse -0.0 with +0.0
            let bits = f.to_bits();
            let monotone = if bits >> 63 == 1 {
                !bits
            } else {
                bits | (1u64 << 63)
            };
            ((1u64 << NORM_CLASS_SHIFT) | (monotone >> 3), exact)
        }
        ValueRef::Text(t) => {
            if !matches!(key.collation, CollationSeq::Unset | CollationSeq::Binary) {
                // Non-binary collation: constant key, always full comparison.
                (2u64 << NORM_CLASS_SHIFT, false)
            } else {
                let bytes = t.value.as_bytes();
                (normalized_prefix(2, bytes), bytes.len() <= 7)
            }
        }
        ValueRef::Blob(b) => (normalized_prefix(3, b), b.len() <= 7),
    };
    decisive &= values.len() == 1 && key_info.len() == 1;
    (
        if key.sort_order == SortOrder::Desc {
            !norm
        } else {
            norm
        },
        decisive,
    )
}

/// Packs the first 7 bytes (big-endian) and a length capped at 8 into the
/// 61-bit payload. With equal prefixes the capped length orders a string
/// against its zero-padded extension correctly, and ties beyond the prefix
/// (both lengths >= 8) fall back to the full comparison.
fn normalized_prefix(class: u64, bytes: &[u8]) -> u64 {
    let mut prefix = [0u8; 8];
    let n = bytes.len().min(7);
    prefix[..n].copy_from_slice(&bytes[..n]);
    let p56 = u64::from_be_bytes(prefix) >> 8;
    (class << NORM_CLASS_SHIFT) | (p56 << 5) | (bytes.len().min(8) as u64)
}

#[derive(Debug, Clone, Copy)]
enum SortState {
    Start,
    Flush,
    InitHeap,
    Next,
}

#[derive(Debug, Clone, Copy)]
enum InsertState {
    Start,
    Insert,
}

#[derive(Debug, Clone, Copy)]
enum InitChunkHeapState {
    Start,
    PushChunk { chunk_idx: usize },
}

/// Amphora: the most rows a sorter keeps when its reader will take only some
/// (`docs/design/executor.md`, decision 5): an ORDER BY's LIMIT plus OFFSET up to this keeps that
/// many rows in a heap; a larger one sorts every row as before, spilling to files, rather than hold
/// that many rows in memory.
pub const MAX_KEPT: usize = 65_536;

/// Amphora: a row a bounded sorter keeps, and when it arrived: rows that compare equal keep their
/// arrival order, as the stable sort of every row keeps it.
struct KeptRecord {
    record: BoxedSortableRecord,
    arrived: u64,
}

impl Ord for KeptRecord {
    fn cmp(&self, other: &Self) -> Ordering {
        self.record
            .cmp(&other.record)
            .then(self.arrived.cmp(&other.arrived))
    }
}

impl PartialOrd for KeptRecord {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for KeptRecord {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for KeptRecord {}

/// Outcome of checking a candidate row against an active top-K heap's byte budget.
enum KeptBudget {
    /// Row orders after the worst kept row and is discarded without allocating.
    Reject,
    /// Row wins a spot in the top-K heap and fits within `Sorter::max_buffer_size`.
    Fits,
    /// Row wins a spot in the top-K heap, and retaining it would exceed `Sorter::max_buffer_size`;
    /// demote the heap into the spilling external sorter.
    Exceeds,
}

/// Amphora: the rows a bounded sorter keeps: the first `limit` in sort order of those offered, in a
/// max-heap, so the worst of them is the one a new row must beat; then, sorted, in reverse output
/// order (served from the end, as `Sorter::records` are).
struct Kept {
    limit: usize,
    heap: BinaryHeap<KeptRecord>,
    /// Sum of `ImmutableRecord::as_blob().capacity()` currently held in `heap`.
    bytes: usize,
    arrived: u64,
    sorted: Vec<KeptRecord>,
}

impl Kept {
    /// Classifies `record` against the heap order and `max_buffer_size` without mutating the heap,
    /// accounting for both payload buffers and `KeptRecord` heap metadata.
    fn classify_insert(
        &self,
        record: &ImmutableRecord,
        key_len: usize,
        index_key_info: &[KeyInfo],
        comparators: &[Option<SortComparator>],
        max_buffer_size: usize,
    ) -> Result<KeptBudget> {
        let payload_len = record.get_payload().len();
        let entry_meta = std::mem::size_of::<KeptRecord>();
        if self.heap.len() >= self.limit {
            let Some(worst) = self.heap.peek() else {
                return Ok(KeptBudget::Fits);
            };
            if !orders_before(record, key_len, index_key_info, comparators, &worst.record)? {
                return Ok(KeptBudget::Reject);
            }
            let evicted_cap = worst.record.record.as_blob().capacity();
            // Reuse in-place ONLY when `evicted_cap >= payload_len` (zero `Vec::reserve` doubling)
            // and `evicted_cap <= 2 * payload_len` (geometric hysteresis). Otherwise allocate an
            // exact-sized buffer (`ImmutableRecord::new` uses `try_reserve_exact`).
            let next_cap =
                if evicted_cap >= payload_len && evicted_cap <= payload_len.saturating_mul(2) {
                    evicted_cap
                } else {
                    payload_len
                };
            let meta_bytes = self.heap.len().saturating_mul(entry_meta);
            let projected = self
                .bytes
                .saturating_sub(evicted_cap)
                .saturating_add(next_cap)
                .saturating_add(meta_bytes);
            if projected > max_buffer_size {
                return Ok(KeptBudget::Exceeds);
            }
        } else {
            let next_len = self.heap.len().saturating_add(1);
            let meta_bytes = next_len.saturating_mul(entry_meta);
            if self
                .bytes
                .saturating_add(payload_len)
                .saturating_add(meta_bytes)
                > max_buffer_size
            {
                return Ok(KeptBudget::Exceeds);
            }
        }
        Ok(KeptBudget::Fits)
    }

    /// Inserts a row already classified as [`KeptBudget::Fits`], reusing the evicted buffer in
    /// place only when it already fits `payload` without growth and stays within 2x of `payload`.
    fn insert_winner(
        &mut self,
        record: &ImmutableRecord,
        key_len: usize,
        index_key_info: &Rc<Vec<KeyInfo>>,
        comparators: &Rc<Vec<Option<SortComparator>>>,
    ) -> Result<()> {
        let arrived = self.arrived;
        self.arrived = self.arrived.saturating_add(1);
        let payload = record.get_payload();
        let copy = if self.heap.len() >= self.limit {
            if let Some(evicted) = self.heap.pop() {
                let mut reused = evicted.record.record;
                let evicted_cap = reused.as_blob().capacity();
                self.bytes = self.bytes.saturating_sub(evicted_cap);
                if evicted_cap >= payload.len() && evicted_cap <= payload.len().saturating_mul(2) {
                    reused.invalidate();
                    reused.start_serialization(payload)?;
                    reused
                } else {
                    drop(reused);
                    let mut copy = ImmutableRecord::new(payload.len())?;
                    copy.start_serialization(payload)?;
                    copy
                }
            } else {
                let mut copy = ImmutableRecord::new(payload.len())?;
                copy.start_serialization(payload)?;
                copy
            }
        } else {
            let mut copy = ImmutableRecord::new(payload.len())?;
            copy.start_serialization(payload)?;
            copy
        };
        let added_cap = copy.as_blob().capacity();
        let record =
            BoxedSortableRecord::new(copy, key_len, index_key_info.clone(), comparators.clone())?;
        self.heap.try_push(KeptRecord { record, arrived })?;
        self.bytes = self.bytes.saturating_add(added_cap);
        Ok(())
    }
}

/// Extracts the first `key_len` column values from a serialized record payload.
fn extract_key_refs<'a>(
    payload: &'a [u8],
    key_len: usize,
) -> Result<smallvec::SmallVec<[ValueRef<'a>; 4]>> {
    let mut refs: smallvec::SmallVec<[ValueRef<'a>; 4]> = smallvec::SmallVec::new();
    if key_len > 4 {
        refs.try_reserve(key_len)
            .map_err(|_| LimboError::OutOfMemory)?;
    }
    let mut iter = ValueIterator::new(payload)?;
    for _ in 0..key_len {
        match iter.next() {
            Some(Ok(v)) => refs.push(v),
            Some(Err(e)) => return Err(e),
            None => crate::bail_corrupt_error!("Not enough columns in record"),
        }
    }
    Ok(refs)
}

fn entry_key_refs(values: &[Value]) -> Result<smallvec::SmallVec<[ValueRef<'_>; 4]>> {
    let mut out: smallvec::SmallVec<[ValueRef<'_>; 4]> = smallvec::SmallVec::new();
    if values.len() > 4 {
        out.try_reserve(values.len())
            .map_err(|_| LimboError::OutOfMemory)?;
    }
    for v in values {
        out.push(v.as_ref());
    }
    Ok(out)
}

/// Fallible full key comparison honoring collations, custom comparators, `ASC`/`DESC`, and
/// `NULLS FIRST`/`NULLS LAST`.
fn try_full_cmp(
    self_values: &[ValueRef<'_>],
    other_values: &[ValueRef<'_>],
    index_key_info: &[KeyInfo],
    comparators: &[Option<SortComparator>],
) -> Result<Ordering> {
    for (i, ((&self_val, &other_val), key_info)) in self_values
        .iter()
        .zip(other_values.iter())
        .zip(index_key_info.iter())
        .enumerate()
    {
        let cmp = if let Some(Some(comparator)) = comparators.get(i) {
            comparator(&self_val, &other_val)?
        } else {
            match (self_val, other_val) {
                (ValueRef::Text(left), ValueRef::Text(right)) => {
                    key_info.collation.compare_strings(&left, &right)
                }
                _ => self_val.partial_cmp(&other_val).unwrap_or(Ordering::Equal),
            }
        };
        let cmp = cmp_with_sort(cmp, &self_val, &other_val, key_info);
        if cmp != Ordering::Equal {
            return Ok(cmp);
        }
    }
    Ok(Ordering::Equal)
}

/// High bit of [`SpilledTopKEntry::arrived_and_decisive`] flags that `norm_key` is decisive.
const TOPK_DECISIVE_BIT: u64 = 1u64 << 63;

fn inline_keys_heap_bytes(inline_keys: &Option<Box<[Value]>>) -> usize {
    let Some(vals) = inline_keys.as_deref() else {
        return 0;
    };
    let mut bytes = vals.len().saturating_mul(std::mem::size_of::<Value>());
    for v in vals {
        bytes = bytes.saturating_add(match v {
            Value::Text(t) => t.as_str().len(),
            Value::Blob(b) => b.capacity(),
            Value::Null | Value::Numeric(_) => 0,
        });
    }
    bytes
}

/// Compact metadata entry held in [`SpilledTopK`] (48 bytes): stores the normalized 64-bit key
/// prefix, arrival rank + `norm_decisive` bit, optional inline sort keys (only while total inline
/// key bytes fit within the remaining budget; wide sort keys leave `inline_keys = None` and
/// resolve rare prefix ties via caller-driven async `pread`), and the reclaimed slot range
/// `[offset, offset + slot_cap)` in the temp file.
struct SpilledTopKEntry {
    inline_keys: Option<Box<[Value]>>,
    norm_key: u64,
    arrived_and_decisive: u64,
    offset: u64,
    payload_len: u32,
    slot_cap: u32,
}

impl SpilledTopKEntry {
    #[inline]
    fn arrived(&self) -> u64 {
        self.arrived_and_decisive & !TOPK_DECISIVE_BIT
    }

    #[inline]
    fn norm_decisive(&self) -> bool {
        (self.arrived_and_decisive & TOPK_DECISIVE_BIT) != 0
    }

    #[inline]
    fn key_bytes(&self) -> usize {
        inline_keys_heap_bytes(&self.inline_keys)
    }

    #[allow(clippy::too_many_arguments)]
    fn from_record(
        record: &ImmutableRecord,
        key_len: usize,
        index_key_info: &[KeyInfo],
        comparators: &[Option<SortComparator>],
        arrived: u64,
        offset: u64,
        slot_cap: usize,
        max_inline_bytes: usize,
    ) -> Result<Self> {
        let payload = record.get_payload();
        let payload_len = u32::try_from(payload.len()).map_err(|_| LimboError::IntegerOverflow)?;
        let slot_cap = u32::try_from(slot_cap).map_err(|_| LimboError::IntegerOverflow)?;
        let refs = extract_key_refs(payload, key_len)?;
        let (norm_key, norm_decisive) = normalized_first_key(&refs, index_key_info, comparators);
        let mut est_key_bytes = key_len.saturating_mul(std::mem::size_of::<Value>());
        for r in &refs {
            est_key_bytes = est_key_bytes.saturating_add(match r {
                ValueRef::Text(t) => t.as_str().len(),
                ValueRef::Blob(b) => b.len(),
                ValueRef::Null | ValueRef::Numeric(_) => 0,
            });
        }
        let inline_keys = if est_key_bytes <= max_inline_bytes {
            let mut owned_vals = Vec::new();
            owned_vals
                .try_reserve_exact(key_len)
                .map_err(|_| LimboError::OutOfMemory)?;
            let mut actual_bytes = key_len.saturating_mul(std::mem::size_of::<Value>());
            for r in &refs {
                let owned = r.to_owned()?;
                actual_bytes = actual_bytes.saturating_add(match &owned {
                    Value::Text(t) => t.as_str().len(),
                    Value::Blob(b) => b.capacity(),
                    Value::Null | Value::Numeric(_) => 0,
                });
                owned_vals.push(owned);
            }
            if actual_bytes <= max_inline_bytes {
                Some(owned_vals.into_boxed_slice())
            } else {
                None
            }
        } else {
            None
        };
        let arrived_and_decisive =
            (arrived & !TOPK_DECISIVE_BIT) | if norm_decisive { TOPK_DECISIVE_BIT } else { 0 };
        Ok(Self {
            inline_keys,
            norm_key,
            arrived_and_decisive,
            offset,
            payload_len,
            slot_cap,
        })
    }
}

struct PendingTopKWrite {
    completion: Completion,
    write_err: Arc<atomic::AtomicBool>,
    entries: VecDeque<SpilledTopKEntry>,
    /// Rows newly counted in `Sorter::inserted` and `SpilledTopK::arrived` once this write settles.
    count_new: u64,
}

struct PendingTopKRead {
    completion: Completion,
    buffer: Arc<Buffer>,
    read_err: Arc<atomic::AtomicBool>,
    expected_len: usize,
}

struct PendingKeyRead {
    offset: u64,
    expected_len: usize,
    completion: Completion,
    buffer: Arc<Buffer>,
    read_err: Arc<atomic::AtomicBool>,
}

/// Caller-driven async key-payload reader for wide-key prefix ties in [`SpilledTopK`].
/// Never blocks with `.wait(io)`: yields `IOResult::IO` on every incomplete `pread`, caches at
/// most the two buffers of the active comparison, and clears them immediately once that
/// comparison finishes.
struct TopKKeyReader {
    file: Arc<dyn File>,
    pending: Option<PendingKeyRead>,
    cache: [Option<(u64, usize, Arc<Buffer>)>; 2],
}

impl TopKKeyReader {
    fn new(file: Arc<dyn File>) -> Self {
        Self {
            file,
            pending: None,
            cache: [None, None],
        }
    }

    fn clear_cache(&mut self) {
        self.cache = [None, None];
    }

    fn store_cached(&mut self, offset: u64, len: usize, buf: Arc<Buffer>) {
        if self.cache[0].is_none() {
            self.cache[0] = Some((offset, len, buf));
        } else if self.cache[1].is_none() {
            self.cache[1] = Some((offset, len, buf));
        } else {
            self.cache[0] = Some((offset, len, buf));
        }
    }

    fn ensure_payload(&mut self, offset: u64, payload_len: usize) -> IOResultOr<Arc<Buffer>> {
        if let Some(pending) = self.pending.take() {
            if !pending.completion.succeeded() {
                let c = pending.completion.clone();
                self.pending = Some(pending);
                return Ok(IOResult::IO(IOCompletions(c)));
            }
            if pending.read_err.load(atomic::Ordering::SeqCst) {
                self.clear_cache();
                return Err(CompletionError::IOError(
                    std::io::ErrorKind::UnexpectedEof,
                    "sorter top-k key read",
                )
                .into());
            }
            self.store_cached(pending.offset, pending.expected_len, pending.buffer);
        }
        for (cached_off, cached_len, buf) in self.cache.iter().flatten() {
            if *cached_off == offset && *cached_len == payload_len {
                return Ok(IOResult::Done(buf.clone()));
            }
        }
        if payload_len == 0 {
            let empty = Arc::new(Buffer::new_temporary(0));
            return Ok(IOResult::Done(empty));
        }
        let read_buf = Arc::new(Buffer::new_temporary(payload_len));
        let expected = i32::try_from(payload_len).map_err(|_| LimboError::IntegerOverflow)?;
        let read_err = Arc::new(atomic::AtomicBool::new(false));
        let read_err_cb = read_err.clone();
        let c = Completion::new_read(
            read_buf.clone(),
            Box::new(move |res: Result<(Arc<Buffer>, i32), CompletionError>| {
                match res {
                    Ok((_, n)) if n >= expected => {}
                    _ => read_err_cb.store(true, atomic::Ordering::SeqCst),
                }
                None
            }),
        );
        let c = self.file.pread(offset, c)?;
        if c.succeeded() {
            if read_err.load(atomic::Ordering::SeqCst) {
                self.clear_cache();
                return Err(CompletionError::IOError(
                    std::io::ErrorKind::UnexpectedEof,
                    "sorter top-k key read",
                )
                .into());
            }
            self.store_cached(offset, payload_len, read_buf.clone());
            return Ok(IOResult::Done(read_buf));
        }
        self.pending = Some(PendingKeyRead {
            offset,
            expected_len: payload_len,
            completion: c.clone(),
            buffer: read_buf,
            read_err,
        });
        Ok(IOResult::IO(IOCompletions(c)))
    }
}

/// Pre-computed, infallible mutation plan for [`SpilledTopK::heap`]: computed strictly read-only
/// against `self.heap` so any async `IO` yield or fallible comparison error leaves `self.heap`
/// completely unmodified and retryable without duplicating rows.
///
/// `ReplaceRoot` encodes up to 64 levels of sift-down turns compactly in a `u64` bitmask
/// (`bit d == 1` picks right child `2*hole + 2`, `0` picks left child `2*hole + 1`).
enum HeapPushPlan {
    InsertAt(usize),
    Reject,
    ReplaceRoot { turns: u64, depth: u8 },
}

/// Resumable read-only sift-down path cursor so async `IOResult::IO` yields resume at the exact
/// comparison without re-reading previously compared entries.
struct SiftDownCursor {
    hole: usize,
    depth: u8,
    turns: u64,
    best_child: Option<usize>,
}

impl SiftDownCursor {
    fn new() -> Self {
        Self {
            hole: 0,
            depth: 0,
            turns: 0,
            best_child: None,
        }
    }

    #[inline]
    fn record_step(&mut self, child: usize) {
        let right = self.hole.saturating_mul(2).saturating_add(2);
        if child == right && self.depth < 64 {
            self.turns |= 1u64 << u32::from(self.depth);
        }
        self.depth = self.depth.saturating_add(1);
        self.hole = child;
    }

    #[inline]
    fn apply_swaps(heap: &mut [SpilledTopKEntry], turns: u64, depth: u8) {
        let mut hole = 0usize;
        for d in 0..u32::from(depth) {
            let child = hole
                .saturating_mul(2)
                .saturating_add(1)
                .saturating_add(usize::from(((turns >> d) & 1) != 0));
            if child >= heap.len() {
                break;
            }
            heap.swap(hole, child);
            hole = child;
        }
    }
}

/// Resumable read-only push-planning cursor for [`SpilledTopK::plan_heap_push`].
enum PlanPushCursor {
    SiftUp { cur: usize },
    CheckRoot,
    SiftDown(SiftDownCursor),
}

/// External top-K selector backed by `Sorter::temp_file`:
/// - Holds `<= limit` compact (48-byte) entries in an explicit fallible binary max-heap, sorted
///   in place with zero extra allocations during `finish_sort_async()`.
/// - Keeps live row-sized RAM within the sorter's budget regardless of input count or key width
///   (`inline_keys` is dropped to `None` when sort keys exceed `max_key_bytes`, resolving rare
///   prefix ties via caller-driven async `pread` without ever blocking `.wait(io)`).
/// - Reclaims evicted winner slots via a coalesced best-fit `free_slots` list so temp-file growth
///   remains `O(K)` (`<= K + 1` active+staging slots) even across arbitrarily many better rows.
/// - Preserves exact SQL sort order (`ASC`/`DESC`, `NULLS FIRST`/`LAST`, collations, custom
///   comparators) and FIFO arrival order on tied keys.
struct SpilledTopK {
    limit: usize,
    max_key_bytes: usize,
    /// Binary max-heap (`heap[0]` is worst) during insert; sorted in place into descending order
    /// `[worst, ..., best]` during `finish_sort_async()` so `heap.pop()` serves output from best
    /// to worst with zero auxiliary allocations.
    heap: Vec<SpilledTopKEntry>,
    /// In-place async heapsort progress: `None` before sorting, `Some(end)` while `heap[0..end]`
    /// is still a max-heap, `Some(0)` once reversed and ready for `read_next_into`.
    sort_end: Option<usize>,
    /// Resumable cursor for read-only heap push planning across async I/O yields.
    plan_push_cursor: Option<PlanPushCursor>,
    /// Resumable cursor for read-only sift-down during `finish_sort_async`.
    sort_sift_cursor: Option<SiftDownCursor>,
    arrived: u64,
    key_bytes: usize,
    key_reader: TopKKeyReader,
    next_offset: u64,
    /// Disjoint, offset-sorted, coalesced free byte ranges `(offset, capacity)` strictly below
    /// `next_offset`.
    free_slots: Vec<(u64, usize)>,
    pending_write: Option<PendingTopKWrite>,
    pending_read: Option<PendingTopKRead>,
}

/// Rounds a temp-file slot length up to cache-line (64 B) or OS-page (4 KiB) granularity so
/// `pread`/`pwrite` offsets stay block-aligned and small varint-width differences between
/// equal-shape rows (e.g. `id = 1` vs `id = 2` in SQLite record format) reuse evicted slots
/// immediately without growing the temp file.
fn aligned_file_slot_cap(len: usize) -> Result<usize> {
    if len == 0 {
        return Ok(0);
    }
    let align = if len >= 4096 { 4096usize } else { 64usize };
    len.checked_next_multiple_of(align)
        .ok_or(LimboError::IntegerOverflow)
}

impl SpilledTopK {
    fn allocated_bytes(&self) -> usize {
        self.key_bytes
            .saturating_add(
                self.heap
                    .capacity()
                    .saturating_mul(std::mem::size_of::<SpilledTopKEntry>()),
            )
            .saturating_add(
                self.free_slots
                    .capacity()
                    .saturating_mul(std::mem::size_of::<(u64, usize)>()),
            )
    }

    /// Reserves a byte slot `[offset, offset + slot_cap)` in `self.key_reader.file`, reusing the
    /// tightest fitting free slot (splitting only when capacity exceeds 2x `aligned_len` so
    /// adjacent-size rows recycle slots without fragmentation) or extending `self.next_offset`.
    fn reserve_file_slot(&mut self, len: usize) -> Result<(u64, usize)> {
        let aligned = aligned_file_slot_cap(len)?;
        if aligned == 0 {
            return Ok((0, 0));
        }
        let mut best_idx: Option<usize> = None;
        let mut best_cap = usize::MAX;
        for (i, &(_, cap)) in self.free_slots.iter().enumerate() {
            if cap >= aligned && cap < best_cap {
                best_cap = cap;
                best_idx = Some(i);
                if cap == aligned {
                    break;
                }
            }
        }
        if let Some(idx) = best_idx {
            let (off, cap) = self.free_slots.remove(idx);
            if cap > aligned.saturating_mul(2) {
                let rem_off = off
                    .checked_add(u64::try_from(aligned).map_err(|_| LimboError::IntegerOverflow)?)
                    .ok_or(LimboError::IntegerOverflow)?;
                let rem_cap = cap.saturating_sub(aligned);
                self.free_slots
                    .try_reserve(1)
                    .map_err(|_| LimboError::OutOfMemory)?;
                self.free_slots.insert(idx, (rem_off, rem_cap));
                return Ok((off, aligned));
            }
            return Ok((off, cap));
        }
        let off = self.next_offset;
        let end = off
            .checked_add(u64::try_from(aligned).map_err(|_| LimboError::IntegerOverflow)?)
            .ok_or(LimboError::IntegerOverflow)?;
        self.next_offset = end;
        Ok((off, aligned))
    }

    /// Returns `[offset, offset + slot_cap)` to `self.free_slots`, coalescing with neighbors and
    /// trimming the file watermark `self.next_offset` whenever the tail becomes free.
    fn release_file_slot(&mut self, offset: u64, slot_cap: usize) {
        if slot_cap == 0 {
            return;
        }
        let new_off = offset;
        let mut new_cap = slot_cap;
        let pos = self.free_slots.partition_point(|&(off, _)| off < new_off);

        // Merge with successor if contiguous.
        if let Some(&(next_off, next_cap)) = self.free_slots.get(pos) {
            if let Some(end) = new_off.checked_add(new_cap as u64) {
                if end == next_off {
                    if let Some(merged) = new_cap.checked_add(next_cap) {
                        new_cap = merged;
                        self.free_slots.remove(pos);
                    }
                }
            }
        }

        // Merge with predecessor if contiguous.
        let mut merged_prev = false;
        if pos > 0 {
            let prev_idx = pos.saturating_sub(1);
            if let Some((prev_off, prev_cap)) = self.free_slots.get_mut(prev_idx) {
                if let Some(prev_end) = prev_off.checked_add(*prev_cap as u64) {
                    if prev_end == new_off {
                        if let Some(merged) = prev_cap.checked_add(new_cap) {
                            *prev_cap = merged;
                            merged_prev = true;
                        }
                    }
                }
            }
        }
        if !merged_prev && self.free_slots.try_reserve(1).is_ok() {
            self.free_slots.insert(pos, (new_off, new_cap));
        }

        // Trim any free tail range back off `self.next_offset`.
        while let Some(&(last_off, last_cap)) = self.free_slots.last() {
            if last_off.checked_add(last_cap as u64) == Some(self.next_offset) {
                self.free_slots.pop();
                self.next_offset = last_off;
            } else {
                break;
            }
        }
    }

    fn compare_entries_async(
        reader: &mut TopKKeyReader,
        a: &SpilledTopKEntry,
        b: &SpilledTopKEntry,
        key_len: usize,
        index_key_info: &[KeyInfo],
        comparators: &[Option<SortComparator>],
    ) -> IOResultOr<Ordering> {
        let key_ord = match a.norm_key.cmp(&b.norm_key) {
            Ordering::Equal if a.norm_decisive() && b.norm_decisive() => Ordering::Equal,
            Ordering::Equal => {
                let a_len = a.payload_len as usize;
                let b_len = b.payload_len as usize;
                let buf_a = if a.inline_keys.is_none() {
                    Some(return_if_io!(reader.ensure_payload(a.offset, a_len)))
                } else {
                    None
                };
                let buf_b = if b.inline_keys.is_none() {
                    Some(return_if_io!(reader.ensure_payload(b.offset, b_len)))
                } else {
                    None
                };
                // Drop temporary read cache entries immediately after both buffers are in hand so
                // no row payloads are ever retained across comparisons.
                reader.clear_cache();
                let refs_a = match (&a.inline_keys, &buf_a) {
                    (Some(vals), _) => entry_key_refs(vals)?,
                    (None, Some(buf)) => {
                        let slice = buf.as_slice().get(..a_len).ok_or(CompletionError::IOError(
                            std::io::ErrorKind::UnexpectedEof,
                            "sorter top-k key read",
                        ))?;
                        extract_key_refs(slice, key_len)?
                    }
                    (None, None) => {
                        return Err(LimboError::InternalError(
                            "spilled top-k entry missing key buffer".into(),
                        )
                        .into());
                    }
                };
                let refs_b = match (&b.inline_keys, &buf_b) {
                    (Some(vals), _) => entry_key_refs(vals)?,
                    (None, Some(buf)) => {
                        let slice = buf.as_slice().get(..b_len).ok_or(CompletionError::IOError(
                            std::io::ErrorKind::UnexpectedEof,
                            "sorter top-k key read",
                        ))?;
                        extract_key_refs(slice, key_len)?
                    }
                    (None, None) => {
                        return Err(LimboError::InternalError(
                            "spilled top-k entry missing key buffer".into(),
                        )
                        .into());
                    }
                };
                try_full_cmp(&refs_a, &refs_b, index_key_info, comparators)?
            }
            ord => ord,
        };
        Ok(IOResult::Done(key_ord.then(a.arrived().cmp(&b.arrived()))))
    }

    fn orders_before_entry_async(
        reader: &mut TopKKeyReader,
        record: &ImmutableRecord,
        worst: &SpilledTopKEntry,
        key_len: usize,
        index_key_info: &[KeyInfo],
        comparators: &[Option<SortComparator>],
    ) -> IOResultOr<bool> {
        let incoming_refs = extract_key_refs(record.get_payload(), key_len)?;
        let (norm_key, norm_decisive) =
            normalized_first_key(&incoming_refs, index_key_info, comparators);
        let ord = match norm_key.cmp(&worst.norm_key) {
            Ordering::Equal if norm_decisive && worst.norm_decisive() => Ordering::Equal,
            Ordering::Equal => {
                let worst_len = worst.payload_len as usize;
                let buf = if worst.inline_keys.is_none() {
                    Some(return_if_io!(reader.ensure_payload(worst.offset, worst_len)))
                } else {
                    None
                };
                reader.clear_cache();
                let worst_refs = match (&worst.inline_keys, &buf) {
                    (Some(vals), _) => entry_key_refs(vals)?,
                    (None, Some(buf)) => {
                        let slice =
                            buf.as_slice()
                                .get(..worst_len)
                                .ok_or(CompletionError::IOError(
                                    std::io::ErrorKind::UnexpectedEof,
                                    "sorter top-k key read",
                                ))?;
                        extract_key_refs(slice, key_len)?
                    }
                    (None, None) => {
                        return Err(LimboError::InternalError(
                            "spilled top-k entry missing key buffer".into(),
                        )
                        .into());
                    }
                };
                try_full_cmp(&incoming_refs, &worst_refs, index_key_info, comparators)?
            }
            ord => ord,
        };
        Ok(IOResult::Done(ord == Ordering::Less))
    }

    /// Advances a resumable read-only sift-down path cursor for `candidate` placed virtually at
    /// index `0` of `heap[0..range_len]` without mutating `heap`.
    #[allow(clippy::too_many_arguments)]
    fn advance_sift_down_range(
        heap: &[SpilledTopKEntry],
        reader: &mut TopKKeyReader,
        cursor: &mut SiftDownCursor,
        range_len: usize,
        candidate: &SpilledTopKEntry,
        key_len: usize,
        index_key_info: &[KeyInfo],
        comparators: &[Option<SortComparator>],
    ) -> IOResultOr<()> {
        loop {
            let best_child = match cursor.best_child {
                Some(child) => child,
                None => {
                    let left = cursor.hole.saturating_mul(2).saturating_add(1);
                    if left >= range_len {
                        break;
                    }
                    let right = left.saturating_add(1);
                    let chosen = if right < range_len {
                        let (Some(l_e), Some(r_e)) = (heap.get(left), heap.get(right)) else {
                            break;
                        };
                        let lr = return_if_io!(Self::compare_entries_async(
                            reader,
                            r_e,
                            l_e,
                            key_len,
                            index_key_info,
                            comparators,
                        ));
                        if lr == Ordering::Greater {
                            right
                        } else {
                            left
                        }
                    } else {
                        left
                    };
                    cursor.best_child = Some(chosen);
                    chosen
                }
            };
            let Some(child_e) = heap.get(best_child) else {
                break;
            };
            let ord = return_if_io!(Self::compare_entries_async(
                reader,
                child_e,
                candidate,
                key_len,
                index_key_info,
                comparators,
            ));
            cursor.best_child = None;
            if ord == Ordering::Greater {
                cursor.record_step(best_child);
            } else {
                break;
            }
        }
        Ok(IOResult::Done(()))
    }

    /// Computes a [`HeapPushPlan`] for `entry` strictly read-only against `self.heap`, resuming
    /// from `self.plan_push_cursor` across async I/O yields so no comparison is ever repeated.
    fn plan_heap_push(
        &mut self,
        entry: &SpilledTopKEntry,
        key_len: usize,
        index_key_info: &[KeyInfo],
        comparators: &[Option<SortComparator>],
    ) -> IOResultOr<HeapPushPlan> {
        let mut cursor = match self.plan_push_cursor.take() {
            Some(c) => c,
            None if self.heap.len() < self.limit => PlanPushCursor::SiftUp {
                cur: self.heap.len(),
            },
            None => PlanPushCursor::CheckRoot,
        };
        loop {
            match &mut cursor {
                PlanPushCursor::SiftUp { cur } => {
                    while *cur > 0 {
                        let parent = cur.saturating_sub(1) / 2;
                        let Some(parent_e) = self.heap.get(parent) else {
                            break;
                        };
                        match Self::compare_entries_async(
                            &mut self.key_reader,
                            entry,
                            parent_e,
                            key_len,
                            index_key_info,
                            comparators,
                        ) {
                            Ok(IOResult::IO(io)) => {
                                self.plan_push_cursor = Some(cursor);
                                return Ok(IOResult::IO(io));
                            }
                            Err(err) => return Err(err),
                            Ok(IOResult::Done(Ordering::Greater)) => *cur = parent,
                            Ok(IOResult::Done(_)) => break,
                        }
                    }
                    return Ok(IOResult::Done(HeapPushPlan::InsertAt(*cur)));
                }
                PlanPushCursor::CheckRoot => {
                    let Some(worst) = self.heap.first() else {
                        return Ok(IOResult::Done(HeapPushPlan::Reject));
                    };
                    match Self::compare_entries_async(
                        &mut self.key_reader,
                        entry,
                        worst,
                        key_len,
                        index_key_info,
                        comparators,
                    ) {
                        Ok(IOResult::IO(io)) => {
                            self.plan_push_cursor = Some(cursor);
                            return Ok(IOResult::IO(io));
                        }
                        Err(err) => return Err(err),
                        Ok(IOResult::Done(Ordering::Less)) => {
                            cursor = PlanPushCursor::SiftDown(SiftDownCursor::new());
                        }
                        Ok(IOResult::Done(_)) => {
                            return Ok(IOResult::Done(HeapPushPlan::Reject));
                        }
                    }
                }
                PlanPushCursor::SiftDown(sift) => {
                    match Self::advance_sift_down_range(
                        &self.heap,
                        &mut self.key_reader,
                        sift,
                        self.heap.len(),
                        entry,
                        key_len,
                        index_key_info,
                        comparators,
                    ) {
                        Ok(IOResult::IO(io)) => {
                            self.plan_push_cursor = Some(cursor);
                            return Ok(IOResult::IO(io));
                        }
                        Err(err) => return Err(err),
                        Ok(IOResult::Done(())) => {
                            return Ok(IOResult::Done(HeapPushPlan::ReplaceRoot {
                                turns: sift.turns,
                                depth: sift.depth,
                            }));
                        }
                    }
                }
            }
        }
    }

    /// Applies a pre-validated [`HeapPushPlan`] infallibly in memory (zero fallible comparisons).
    fn apply_heap_push_plan(&mut self, entry: SpilledTopKEntry, plan: HeapPushPlan) {
        match plan {
            HeapPushPlan::InsertAt(target) => {
                let added = entry.key_bytes();
                let mut pos = self.heap.len();
                // Capacity was pre-reserved before submitting disk I/O.
                self.heap.push(entry);
                while pos > target {
                    let parent = pos.saturating_sub(1) / 2;
                    self.heap.swap(pos, parent);
                    pos = parent;
                }
                self.key_bytes = self.key_bytes.saturating_add(added);
            }
            HeapPushPlan::Reject => {
                self.release_file_slot(entry.offset, entry.slot_cap as usize);
            }
            HeapPushPlan::ReplaceRoot { turns, depth } => {
                let added = entry.key_bytes();
                let Some(root) = self.heap.first_mut() else {
                    self.release_file_slot(entry.offset, entry.slot_cap as usize);
                    return;
                };
                let evicted = std::mem::replace(root, entry);
                SiftDownCursor::apply_swaps(&mut self.heap, turns, depth);
                self.key_bytes = self
                    .key_bytes
                    .saturating_sub(evicted.key_bytes())
                    .saturating_add(added);
                self.release_file_slot(evicted.offset, evicted.slot_cap as usize);
            }
        }
    }

    /// Sorts `self.heap` in place into descending order `[worst, ..., best]` (so `read_next_into`
    /// pops from the back in ascending output order) with ZERO auxiliary heap allocations,
    /// caller-driven async I/O, and zero partial heap corruption on comparison error.
    fn finish_sort_async(
        &mut self,
        key_len: usize,
        index_key_info: &[KeyInfo],
        comparators: &[Option<SortComparator>],
    ) -> IOResultOr<()> {
        let mut end = match self.sort_end {
            Some(0) => return Ok(IOResult::Done(())),
            Some(e) => e,
            None => {
                let len = self.heap.len();
                self.sort_end = Some(len);
                len
            }
        };
        while end > 1 {
            let last_idx = end.saturating_sub(1);
            let Some(candidate) = self.heap.get(last_idx) else {
                break;
            };
            let mut cursor = self
                .sort_sift_cursor
                .take()
                .unwrap_or_else(SiftDownCursor::new);
            match Self::advance_sift_down_range(
                &self.heap,
                &mut self.key_reader,
                &mut cursor,
                last_idx,
                candidate,
                key_len,
                index_key_info,
                comparators,
            ) {
                Ok(IOResult::IO(io)) => {
                    self.sort_sift_cursor = Some(cursor);
                    return Ok(IOResult::IO(io));
                }
                Err(err) => return Err(err),
                Ok(IOResult::Done(())) => {}
            }
            // All comparisons for this extraction succeeded: move max (`heap[0]`) to `last_idx`
            // and shift along `cursor` so the former `heap[last_idx]` lands at its hole.
            self.heap.swap(0, last_idx);
            if let Some(prefix) = self.heap.get_mut(..last_idx) {
                SiftDownCursor::apply_swaps(prefix, cursor.turns, cursor.depth);
            }
            end = last_idx;
            self.sort_end = Some(end);
        }
        // `self.heap` is now `[best, ..., worst]`; reverse in place so `pop()` yields best first.
        self.heap.reverse();
        self.sort_end = Some(0);
        Ok(IOResult::Done(()))
    }

    fn poll_pending_write(
        &mut self,
        key_len: usize,
        index_key_info: &[KeyInfo],
        comparators: &[Option<SortComparator>],
    ) -> IOResultOr<u64> {
        let Some(mut pending) = self.pending_write.take() else {
            return Ok(IOResult::Done(0));
        };
        if !pending.completion.succeeded() {
            let c = pending.completion.clone();
            self.pending_write = Some(pending);
            return Ok(IOResult::IO(IOCompletions(c)));
        }
        if pending.write_err.load(atomic::Ordering::SeqCst) {
            for entry in pending.entries {
                self.release_file_slot(entry.offset, entry.slot_cap as usize);
            }
            return Err(CompletionError::IOError(
                std::io::ErrorKind::WriteZero,
                "sorter top-k write",
            )
            .into());
        }
        while let Some(entry_ref) = pending.entries.front() {
            match self.plan_heap_push(entry_ref, key_len, index_key_info, comparators) {
                Ok(IOResult::IO(io)) => {
                    self.pending_write = Some(pending);
                    return Ok(IOResult::IO(io));
                }
                Err(err) => {
                    for remaining in pending.entries {
                        self.release_file_slot(remaining.offset, remaining.slot_cap as usize);
                    }
                    return Err(err);
                }
                Ok(IOResult::Done(plan)) => {
                    if let Some(entry) = pending.entries.pop_front() {
                        self.apply_heap_push_plan(entry, plan);
                    }
                }
            }
        }
        self.arrived = self.arrived.saturating_add(pending.count_new);
        Ok(IOResult::Done(pending.count_new))
    }

    fn write_slotted_payloads(
        file: &Arc<dyn File>,
        start_offset: u64,
        total_len: usize,
        items: &[(u64, &[u8])],
    ) -> Result<(Completion, Arc<atomic::AtomicBool>, u64)> {
        let end_offset = start_offset
            .checked_add(u64::try_from(total_len).map_err(|_| LimboError::IntegerOverflow)?)
            .ok_or(LimboError::IntegerOverflow)?;
        let buffer = Buffer::new_temporary(total_len);
        let dst = buffer.as_mut_slice();
        for &(item_off, p) in items {
            if !p.is_empty() {
                let rel = usize::try_from(
                    item_off
                        .checked_sub(start_offset)
                        .ok_or(LimboError::IntegerOverflow)?,
                )
                .map_err(|_| LimboError::IntegerOverflow)?;
                let end = rel
                    .checked_add(p.len())
                    .ok_or(LimboError::IntegerOverflow)?;
                let slot = dst
                    .get_mut(rel..end)
                    .ok_or_else(|| LimboError::InternalError("sorter top-k write slice".into()))?;
                slot.copy_from_slice(p);
            }
        }
        let buffer_ref = Arc::new(buffer);
        let expected = i32::try_from(total_len).map_err(|_| LimboError::IntegerOverflow)?;
        let write_err = Arc::new(atomic::AtomicBool::new(false));
        let write_err_cb = write_err.clone();
        let c = Completion::new_write(Box::new(
            move |res: Result<i32, CompletionError>| match res {
                Ok(n) if n >= expected => {}
                _ => write_err_cb.store(true, atomic::Ordering::SeqCst),
            },
        ));
        let c = file.pwrite(start_offset, buffer_ref, c)?;
        Ok((c, write_err, end_offset))
    }

    fn offer(
        &mut self,
        record: &ImmutableRecord,
        key_len: usize,
        index_key_info: &[KeyInfo],
        comparators: &[Option<SortComparator>],
    ) -> IOResultOr<u64> {
        let added = return_if_io!(self.poll_pending_write(key_len, index_key_info, comparators));
        if added > 0 {
            return Ok(IOResult::Done(added));
        }
        let mut evicted_key_bytes = 0usize;
        if self.heap.len() >= self.limit {
            if let Some(worst) = self.heap.first() {
                let beats = return_if_io!(Self::orders_before_entry_async(
                    &mut self.key_reader,
                    record,
                    worst,
                    key_len,
                    index_key_info,
                    comparators,
                ));
                if !beats {
                    self.arrived = self.arrived.saturating_add(1);
                    return Ok(IOResult::Done(1));
                }
                evicted_key_bytes = worst.key_bytes();
            }
        } else {
            // Reserve heap capacity before submitting disk I/O so commit cannot OOM.
            self.heap
                .try_reserve(1)
                .map_err(|_| LimboError::OutOfMemory)?;
        }
        let payload = record.get_payload();
        let available_inline = self
            .max_key_bytes
            .saturating_sub(self.key_bytes.saturating_sub(evicted_key_bytes));
        let (slot_offset, slot_cap) = self.reserve_file_slot(payload.len())?;
        let entry = match SpilledTopKEntry::from_record(
            record,
            key_len,
            index_key_info,
            comparators,
            self.arrived,
            slot_offset,
            slot_cap,
            available_inline,
        ) {
            Ok(e) => e,
            Err(err) => {
                self.release_file_slot(slot_offset, slot_cap);
                return Err(err.into());
            }
        };
        let mut entries = VecDeque::new();
        if entries.try_reserve_exact(1).is_err() {
            self.release_file_slot(slot_offset, slot_cap);
            return Err(LimboError::OutOfMemory.into());
        }
        entries.push_back(entry);
        let (completion, write_err, _) = match Self::write_slotted_payloads(
            &self.key_reader.file,
            slot_offset,
            payload.len(),
            &[(slot_offset, payload)],
        ) {
            Ok(res) => res,
            Err(err) => {
                self.release_file_slot(slot_offset, slot_cap);
                return Err(err.into());
            }
        };
        self.pending_write = Some(PendingTopKWrite {
            completion,
            write_err,
            entries,
            count_new: 1,
        });
        self.poll_pending_write(key_len, index_key_info, comparators)
    }

    fn read_next_into(&mut self, current: &mut Option<ImmutableRecord>) -> IOResultOr<()> {
        if let Some(pending) = self.pending_read.take() {
            if !pending.completion.succeeded() {
                let c = pending.completion.clone();
                self.pending_read = Some(pending);
                return Ok(IOResult::IO(IOCompletions(c)));
            }
            if pending.read_err.load(atomic::Ordering::SeqCst) {
                return Err(CompletionError::IOError(
                    std::io::ErrorKind::UnexpectedEof,
                    "sorter top-k read",
                )
                .into());
            }
            let bytes = pending
                .buffer
                .as_slice()
                .get(..pending.expected_len)
                .ok_or(CompletionError::IOError(
                    std::io::ErrorKind::UnexpectedEof,
                    "sorter top-k read",
                ))?;
            match current {
                Some(rec)
                    if rec.as_blob().capacity() >= bytes.len()
                        && rec.as_blob().capacity() <= bytes.len().saturating_mul(2) =>
                {
                    rec.invalidate();
                    rec.start_serialization(bytes)?;
                }
                _ => {
                    let mut rec = ImmutableRecord::new(bytes.len())?;
                    rec.start_serialization(bytes)?;
                    *current = Some(rec);
                }
            }
            // Pop only after the read succeeded and `*current` has been populated so a read
            // failure never loses the winner on retry.
            if let Some(done) = self.heap.pop() {
                self.key_bytes = self.key_bytes.saturating_sub(done.key_bytes());
            }
            return Ok(IOResult::Done(()));
        }

        let Some(entry) = self.heap.last() else {
            *current = None;
            return Ok(IOResult::Done(()));
        };
        let payload_len = entry.payload_len as usize;
        if payload_len == 0 {
            let mut rec = ImmutableRecord::new(0)?;
            rec.start_serialization(&[])?;
            *current = Some(rec);
            if let Some(done) = self.heap.pop() {
                self.key_bytes = self.key_bytes.saturating_sub(done.key_bytes());
            }
            return Ok(IOResult::Done(()));
        }
        let read_buf = Arc::new(Buffer::new_temporary(payload_len));
        let expected = i32::try_from(payload_len).map_err(|_| LimboError::IntegerOverflow)?;
        let read_err = Arc::new(atomic::AtomicBool::new(false));
        let read_err_cb = read_err.clone();
        let c = Completion::new_read(
            read_buf.clone(),
            Box::new(move |res: Result<(Arc<Buffer>, i32), CompletionError>| {
                match res {
                    Ok((_, n)) if n >= expected => {}
                    _ => read_err_cb.store(true, atomic::Ordering::SeqCst),
                }
                None
            }),
        );
        let c = self.key_reader.file.pread(entry.offset, c)?;
        self.pending_read = Some(PendingTopKRead {
            completion: c,
            buffer: read_buf,
            read_err,
            expected_len: payload_len,
        });
        self.read_next_into(current)
    }
}

/// Amphora: whether `record`'s sort key orders strictly before `kept`'s, as the boxed records'
/// comparison orders them (a row tying with the worst kept row arrived later, so it doesn't).
/// Uses inline stack storage for up to 4 sort-key columns to avoid heap allocation per rejected row.
fn orders_before(
    record: &ImmutableRecord,
    key_len: usize,
    index_key_info: &[KeyInfo],
    comparators: &[Option<SortComparator>],
    kept: &BoxedSortableRecord,
) -> Result<bool> {
    if kept.deserialization_error.is_some() {
        return Ok(false);
    }
    let mut values: smallvec::SmallVec<[ValueRef<'_>; 4]> = smallvec::SmallVec::new();
    if key_len > 4 {
        values
            .try_reserve(key_len)
            .map_err(|_| LimboError::OutOfMemory)?;
    }
    let mut iter = record.iter()?;
    for _ in 0..key_len {
        match iter.next() {
            Some(Ok(value)) => values.push(value),
            Some(Err(e)) => return Err(e),
            None => crate::bail_corrupt_error!("Not enough columns in record"),
        }
    }
    let (norm_key, norm_decisive) = normalized_first_key(&values, index_key_info, comparators);
    let ord = match norm_key.cmp(&kept.norm_key) {
        Ordering::Equal if norm_decisive && kept.norm_decisive => Ordering::Equal,
        Ordering::Equal => boxed_full_cmp(&values, &kept.key_values, index_key_info, comparators),
        ord => ord,
    };
    Ok(ord == Ordering::Less)
}

pub struct Sorter {
    /// Arena allocator for records - provides fast bump allocation and bulk deallocation.
    /// All record data (payload bytes, key_values) is stored here for in-memory sorting.
    arena: Bump,
    /// Pointers to records allocated in the arena. Sorting moves only 8-byte pointers,
    /// which prevents high memmove costs during sorting.
    /// SAFETY: These pointers are valid as long as the arena hasn't been reset.
    records: Vec<NonNull<ArenaSortableRecord>>,
    /// The current record.
    current: Option<ImmutableRecord>,
    /// The number of values in the key.
    key_len: usize,
    /// The key info.
    pub index_key_info: Rc<Vec<KeyInfo>>,
    /// Per-column custom comparators for custom type ordering.
    /// When present, used instead of standard ValueRef comparison for that column.
    comparators: Rc<Vec<Option<SortComparator>>>,
    /// Sorted chunks stored on disk.
    chunks: Vec<SortedChunk>,
    /// The heap of records consumed from the chunks and their corresponding chunk index
    /// (`Reverse(chunk_idx)` breaks equal-key ties in favor of the earlier spill run to preserve
    /// FIFO arrival order across spilled chunks).
    chunk_heap: BinaryHeap<(Reverse<Box<BoxedSortableRecord>>, Reverse<usize>)>,
    /// The maximum size of the in-memory buffer in bytes before the records are flushed to a chunk file.
    max_buffer_size: usize,
    /// The current size of the in-memory buffer in bytes.
    current_buffer_size: usize,
    /// The minimum size of a chunk read buffer in bytes. The actual buffer size can be larger if the largest
    /// record in the buffer is larger than this value.
    min_chunk_read_buffer_size: usize,
    /// The maximum record payload size in the in-memory buffer.
    max_payload_size_in_buffer: usize,
    /// The IO object.
    io: Arc<dyn IO>,
    /// The temporary file for chunks.
    temp_file: Option<TempFile>,
    /// Offset where the next chunk will be placed in the `temp_file`
    next_chunk_offset: usize,
    /// State machine for [Sorter::sort]
    sort_state: SortState,
    /// State machine for [Sorter::insert]
    insert_state: InsertState,
    /// State machine for [Sorter::init_chunk_heap]
    init_chunk_heap_state: InitChunkHeapState,
    /// Pending IO completion along with the chunk index that needs to be retried after IO completes.
    pending_completion: Option<(Completion, usize)>,
    /// Temp storage mode (memory vs file) for spilled data
    temp_store: crate::TempStore,
    /// Amphora: the rows kept when the reader takes only some (decision 5), else None.
    kept: Option<Kept>,
    /// File-backed external top-K heap used when `kept` overflows `max_buffer_size`, keeping live
    /// row-sized memory `O(1)` and temp-file usage `O(K)` regardless of input size or key width.
    spilled_top_k: Option<SpilledTopK>,
    /// In-flight conversion from `kept` to `spilled_top_k`. `self.kept` stays completely intact
    /// until this demotion's disk write and heap insertion finish cleanly, so a short write or
    /// allocation failure never loses previously accepted winners.
    pending_demote: Option<SpilledTopK>,
    /// In-flight conversion from `kept` directly to chunk 0 when `limit * size_of::<SpilledTopKEntry>()`
    /// itself exceeds `max_buffer_size` (Standards P1-3). Keeps `self.kept` intact until chunk 0
    /// settles on disk.
    pending_chunk_demote: Option<(Completion, SortedChunk)>,
    /// Optional row cap applied when chunked external merge serves a large-LIMIT query.
    output_limit: Option<usize>,
    /// Rows already yielded to the caller from chunked merge sort.
    emitted: usize,
    /// Amphora: whether the first insert has decided `kept`.
    limit_decided: bool,
    /// Amphora (PL.13, lane replan-5): every row put in, kept or not, in memory or spilled.
    inserted: u64,
    /// Amphora (QT.5): the rows the last in-memory sort ordered and the nanoseconds it took, for
    /// the database's learned sort cost (`take_in_memory_sort`).
    in_memory_sort: Option<(usize, f64)>,
}

impl Sorter {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        order: &[SortOrder],
        collations: Vec<CollationSeq>,
        nulls_orders: Vec<Option<amphora_engine_parser::ast::NullsOrder>>,
        comparators: Vec<Option<SortComparator>>,
        max_buffer_size_bytes: usize,
        min_chunk_read_buffer_size_bytes: usize,
        io: Arc<dyn IO>,
        temp_store: crate::TempStore,
    ) -> Result<Self> {
        turso_assert_eq!(order.len(), collations.len());
        let index_key_info = order
            .iter()
            .zip(collations)
            .zip(nulls_orders)
            .map(|((order, collation), nulls)| KeyInfo {
                sort_order: *order,
                collation,
                nulls_order: nulls,
            })
            .try_collect()?;
        let this = Self {
            arena: Bump::new(),
            records: vec![],
            current: None,
            key_len: order.len(),
            index_key_info: Rc::new(index_key_info),
            comparators: Rc::new(comparators),
            chunks: vec![],
            chunk_heap: TursoAllocExt::new(),
            max_buffer_size: max_buffer_size_bytes,
            current_buffer_size: 0,
            min_chunk_read_buffer_size: min_chunk_read_buffer_size_bytes,
            max_payload_size_in_buffer: 0,
            io,
            temp_file: None,
            next_chunk_offset: 0,
            sort_state: SortState::Start,
            insert_state: InsertState::Start,
            init_chunk_heap_state: InitChunkHeapState::Start,
            pending_completion: None,
            temp_store,
            kept: None,
            spilled_top_k: None,
            pending_demote: None,
            pending_chunk_demote: None,
            output_limit: None,
            emitted: 0,
            limit_decided: false,
            inserted: 0,
            in_memory_sort: None,
        };
        Ok(this)
    }

    /// Opens or reuses the sorter's temp file. Under `TempStore::Default`, falls back to in-memory
    /// temp storage if creating a disk temp file fails (e.g. missing/unwritable `TMPDIR` or full
    /// temp filesystem — Blocker 3), while `TempStore::File` strictly propagates I/O errors.
    fn open_temp_file(&mut self) -> Result<Arc<dyn File>> {
        if let Some(tf) = &self.temp_file {
            return Ok(tf.file.clone());
        }
        let tf = match TempFile::with_temp_store(&self.io, self.temp_store) {
            Ok(tf) => tf,
            Err(_) if matches!(self.temp_store, crate::TempStore::Default) => {
                TempFile::with_temp_store(&self.io, crate::TempStore::Memory)?
            }
            Err(err) => return Err(err),
        };
        let file = tf.file.clone();
        self.temp_file = Some(tf);
        Ok(file)
    }

    /// Heap bytes currently retained by this sorter's bump arena, pointer slab, top-K heap,
    /// spilled top-K metadata, and chunk read buffers.
    pub fn allocated_bytes(&self) -> usize {
        let kept_bytes = self.kept.as_ref().map_or(0, |kept| {
            let sorted_bytes: usize = kept
                .sorted
                .iter()
                .map(|r| r.record.record.as_blob().capacity())
                .sum();
            let meta_bytes = kept
                .heap
                .capacity()
                .saturating_add(kept.sorted.capacity())
                .saturating_mul(std::mem::size_of::<KeptRecord>());
            kept.bytes
                .saturating_add(sorted_bytes)
                .saturating_add(meta_bytes)
        });
        let spilled_top_k_bytes = self
            .spilled_top_k
            .as_ref()
            .map_or(0, SpilledTopK::allocated_bytes)
            .saturating_add(
                self.pending_demote
                    .as_ref()
                    .map_or(0, SpilledTopK::allocated_bytes),
            );
        let chunk_bytes: usize = self
            .chunks
            .iter()
            .map(|c| {
                c.buffer
                    .read()
                    .capacity()
                    .saturating_add(c.staged.as_ref().map_or(0, |r| r.as_blob().capacity()))
            })
            .sum();
        self.arena
            .allocated_bytes()
            .saturating_add(
                self.records
                    .capacity()
                    .saturating_mul(std::mem::size_of::<NonNull<ArenaSortableRecord>>()),
            )
            .saturating_add(kept_bytes)
            .saturating_add(spilled_top_k_bytes)
            .saturating_add(chunk_bytes)
    }

    /// True when this sorter has not spilled to disk and its retained arena/vector capacity fits
    /// within [`crate::vdbe::KEPT_EPHEMERAL_BYTES`] so idle cached statements never retain large
    /// uncharged sort buffers across resets.
    pub fn can_reuse(&self) -> bool {
        self.chunks.is_empty()
            && self.spilled_top_k.is_none()
            && self.pending_demote.is_none()
            && self.pending_chunk_demote.is_none()
            && self.temp_file.is_none()
            && self.pending_completion.is_none()
            && self.allocated_bytes() <= crate::vdbe::KEPT_EPHEMERAL_BYTES
    }

    /// Clears rows and resets the bump arena while dropping all top-K record payloads so idle
    /// cached statements never retain prior rows or blobs outside the statement cache budget.
    pub fn clear_for_reuse(&mut self) {
        self.records.clear();
        if self.arena.allocated_bytes() > self.max_buffer_size {
            self.arena = Bump::new();
        } else {
            self.arena.reset();
        }
        self.current = None;
        self.current_buffer_size = 0;
        self.max_payload_size_in_buffer = 0;
        self.sort_state = SortState::Start;
        self.insert_state = InsertState::Start;
        self.init_chunk_heap_state = InitChunkHeapState::Start;
        self.kept = None;
        self.spilled_top_k = None;
        self.pending_demote = None;
        self.pending_chunk_demote = None;
        self.output_limit = None;
        self.emitted = 0;
        self.limit_decided = false;
        self.inserted = 0;
        self.in_memory_sort = None;
    }

    /// Reconfigures an in-memory sorter for a new `SorterOpen` without reallocating its arena or
    /// vectors.
    #[allow(clippy::too_many_arguments)]
    pub fn reconfigure_for_reuse(
        &mut self,
        order: &[SortOrder],
        collations: Vec<CollationSeq>,
        nulls_orders: Vec<Option<amphora_engine_parser::ast::NullsOrder>>,
        comparators: Vec<Option<SortComparator>>,
        max_buffer_size_bytes: usize,
        min_chunk_read_buffer_size_bytes: usize,
        io: Arc<dyn IO>,
        temp_store: crate::TempStore,
    ) -> Result<()> {
        self.clear_for_reuse();
        self.key_len = order.len();
        let index_key_info: Vec<KeyInfo> = order
            .iter()
            .zip(collations)
            .zip(nulls_orders)
            .map(|((order, collation), nulls)| KeyInfo {
                sort_order: *order,
                collation,
                nulls_order: nulls,
            })
            .try_collect()?;
        self.index_key_info = Rc::new(index_key_info);
        self.comparators = Rc::new(comparators);
        self.max_buffer_size = max_buffer_size_bytes;
        self.min_chunk_read_buffer_size = min_chunk_read_buffer_size_bytes;
        self.io = io;
        self.temp_store = temp_store;
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
            && self.chunks.is_empty()
            && self.pending_demote.is_none()
            && self.pending_chunk_demote.is_none()
            && self
                .kept
                .as_ref()
                .filter(|kept| kept.limit > 0)
                .is_none_or(|kept| kept.heap.is_empty() && kept.sorted.is_empty())
            && self
                .spilled_top_k
                .as_ref()
                .is_none_or(|s| s.heap.is_empty() && s.pending_write.is_none())
    }

    /// Amphora: keeps only the first `limit` rows in sort order from here on (decision 5), if no
    /// row is in yet and `limit` is within [`MAX_KEPT`]; asked before each insert, decided at the
    /// first.
    pub fn limit_to(&mut self, limit: Option<usize>) {
        if self.limit_decided {
            return;
        }
        self.limit_decided = true;
        if let Some(limit) = limit.filter(|&l| l > 0) {
            self.output_limit = Some(limit);
            let spilled_meta = limit.saturating_mul(std::mem::size_of::<SpilledTopKEntry>());
            if limit <= MAX_KEPT
                && spilled_meta <= self.max_buffer_size
                && self.records.is_empty()
                && self.chunks.is_empty()
            {
                self.kept = Some(Kept {
                    limit,
                    heap: BinaryHeap::new(),
                    bytes: 0,
                    arrived: 0,
                    sorted: Vec::new(),
                });
            }
        }
    }

    /// Completes any in-flight conversion from `self.kept` to `self.spilled_top_k`, committing
    /// the transition (and dropping `self.kept`) ONLY once disk write verification and every heap
    /// insertion succeed. On error, `self.pending_demote` is dropped while `self.kept` remains
    /// completely intact for retry.
    fn poll_pending_demote(&mut self) -> IOResultOr<bool> {
        let Some(mut spilled) = self.pending_demote.take() else {
            return Ok(IOResult::Done(false));
        };
        match spilled.poll_pending_write(self.key_len, &self.index_key_info, &self.comparators) {
            Ok(IOResult::IO(io)) => {
                self.pending_demote = Some(spilled);
                Ok(IOResult::IO(io))
            }
            Err(err) => Err(err),
            Ok(IOResult::Done(added)) => {
                self.kept = None;
                self.spilled_top_k = Some(spilled);
                self.inserted = self.inserted.saturating_add(added);
                Ok(IOResult::Done(true))
            }
        }
    }

    /// Completes any in-flight conversion from `self.kept` to chunk 0 for large-LIMIT queries whose
    /// `SpilledTopK` metadata alone exceeds `self.max_buffer_size` (Standards P1-3).
    fn poll_pending_chunk_demote(&mut self) -> IOResultOr<bool> {
        let Some((completion, chunk)) = self.pending_chunk_demote.take() else {
            return Ok(IOResult::Done(false));
        };
        if !completion.succeeded() {
            self.pending_chunk_demote = Some((completion.clone(), chunk));
            return Ok(IOResult::IO(IOCompletions(completion)));
        }
        if matches!(*chunk.io_state.read(), SortedChunkIOState::WriteError) {
            return Err(CompletionError::IOError(
                std::io::ErrorKind::WriteZero,
                "sorter chunk demote write",
            )
            .into());
        }
        let chunk_size = chunk.chunk_size;
        self.chunks.try_push(chunk)?;
        self.next_chunk_offset = self.next_chunk_offset.saturating_add(chunk_size);
        self.kept = None;
        Ok(IOResult::Done(true))
    }

    /// Spills `self.kept` as chunk 0 so a large-LIMIT sort that exceeds `max_buffer_size` falls
    /// back to budget-bounded chunked external merge sort (with `output_limit`) instead of
    /// allocating an oversized `SpilledTopK` heap.
    fn demote_kept_to_chunks(&mut self) -> IOResultOr<()> {
        if self.kept.as_ref().is_none_or(|k| k.heap.is_empty()) {
            self.kept = None;
            return Ok(IOResult::Done(()));
        }
        let file = self.open_temp_file()?;
        let (completion, chunk) = {
            let Some(kept) = self.kept.as_ref() else {
                return Ok(IOResult::Done(()));
            };
            let mut refs: Vec<&KeptRecord> = Vec::new();
            refs.try_reserve_exact(kept.heap.len())
                .map_err(|_| LimboError::OutOfMemory)?;
            refs.extend(kept.heap.iter());
            refs.sort_unstable();

            let mut max_payload = 0usize;
            let mut chunk_size = 0usize;
            let mut record_size_lengths: Vec<usize> = Vec::new();
            record_size_lengths
                .try_reserve_exact(refs.len())
                .map_err(|_| LimboError::OutOfMemory)?;
            for row in &refs {
                let plen = row.record.record.get_payload().len();
                max_payload = max_payload.max(plen);
                let slen = varint_len(plen as u64);
                record_size_lengths.push(slen);
                chunk_size = chunk_size
                    .checked_add(slen)
                    .and_then(|s| s.checked_add(plen))
                    .ok_or(LimboError::IntegerOverflow)?;
            }

            let min_buf = max_payload.saturating_add(9);
            let mut chunk = SortedChunk::new(file, self.next_chunk_offset, min_buf)?;
            let completion = chunk.write_kept_records(&refs, &record_size_lengths, chunk_size)?;
            (completion, chunk)
        };
        self.pending_chunk_demote = Some((completion, chunk));
        let res = self.poll_pending_chunk_demote();
        // Under `TempStore::Default`, if the first write to a disk-backed temp file failed (e.g.
        // full temp filesystem), retry once on in-memory temp storage while `self.kept` is intact.
        if res.is_err()
            && self.chunks.is_empty()
            && matches!(self.temp_store, crate::TempStore::Default)
            && self
                .temp_file
                .as_ref()
                .is_some_and(TempFile::is_disk_backed)
        {
            self.temp_file = Some(TempFile::with_temp_store(
                &self.io,
                crate::TempStore::Memory,
            )?);
            return self.demote_kept_to_chunks();
        }
        let _ = return_if_io!(res);
        Ok(IOResult::Done(()))
    }

    /// Begins spilling `self.kept` plus `incoming` into an external [`SpilledTopK`] backed by
    /// `self.temp_file`, keeping `self.kept` intact until [`Self::poll_pending_demote`] confirms
    /// the write and all heap insertions succeeded.
    fn demote_kept_to_spilled_top_k(&mut self, incoming: &ImmutableRecord) -> IOResultOr<()> {
        if self.kept.is_none() {
            return Ok(IOResult::Done(()));
        }
        let file = self.open_temp_file()?;
        let spilled = {
            let Some(kept) = self.kept.as_ref() else {
                return Ok(IOResult::Done(()));
            };
            let limit = kept.limit;
            let spilled_meta_bytes = limit.saturating_mul(std::mem::size_of::<SpilledTopKEntry>());
            let max_key_bytes = self.max_buffer_size.saturating_sub(spilled_meta_bytes);

            let mut refs: Vec<&KeptRecord> = Vec::new();
            refs.try_reserve_exact(kept.heap.len())
                .map_err(|_| LimboError::OutOfMemory)?;
            refs.extend(kept.heap.iter());
            refs.sort_unstable_by_key(|row| row.arrived);

            let total_rows = refs.len().saturating_add(1);
            let mut entries: VecDeque<SpilledTopKEntry> = VecDeque::new();
            entries
                .try_reserve_exact(total_rows)
                .map_err(|_| LimboError::OutOfMemory)?;
            let mut payloads: Vec<(u64, &[u8])> = Vec::new();
            payloads
                .try_reserve_exact(total_rows)
                .map_err(|_| LimboError::OutOfMemory)?;

            let mut offset = 0u64;
            let mut remaining_inline = max_key_bytes;
            for row in &refs {
                let plen = row.record.record.get_payload().len();
                let slot_cap = aligned_file_slot_cap(plen)?;
                let entry = SpilledTopKEntry::from_record(
                    &row.record.record,
                    self.key_len,
                    &self.index_key_info,
                    &self.comparators,
                    row.arrived,
                    offset,
                    slot_cap,
                    remaining_inline,
                )?;
                remaining_inline = remaining_inline.saturating_sub(entry.key_bytes());
                payloads.push((offset, row.record.record.get_payload()));
                offset = offset
                    .checked_add(u64::try_from(slot_cap).map_err(|_| LimboError::IntegerOverflow)?)
                    .ok_or(LimboError::IntegerOverflow)?;
                entries.push_back(entry);
            }
            let incoming_len = incoming.get_payload().len();
            let incoming_cap = aligned_file_slot_cap(incoming_len)?;
            let incoming_entry = SpilledTopKEntry::from_record(
                incoming,
                self.key_len,
                &self.index_key_info,
                &self.comparators,
                kept.arrived,
                offset,
                incoming_cap,
                remaining_inline,
            )?;
            let write_len = usize::try_from(
                offset
                    .checked_add(
                        u64::try_from(incoming_len).map_err(|_| LimboError::IntegerOverflow)?,
                    )
                    .ok_or(LimboError::IntegerOverflow)?,
            )
            .map_err(|_| LimboError::IntegerOverflow)?;
            let end_offset = offset
                .checked_add(u64::try_from(incoming_cap).map_err(|_| LimboError::IntegerOverflow)?)
                .ok_or(LimboError::IntegerOverflow)?;
            payloads.push((offset, incoming.get_payload()));
            entries.push_back(incoming_entry);
            let kept_arrived = kept.arrived;

            let mut heap: Vec<SpilledTopKEntry> = Vec::new();
            heap.try_reserve_exact(limit.min(total_rows))
                .map_err(|_| LimboError::OutOfMemory)?;
            let (completion, write_err, written_end) =
                SpilledTopK::write_slotted_payloads(&file, 0, write_len, &payloads)?;
            let next_offset = end_offset.max(written_end);

            SpilledTopK {
                limit,
                max_key_bytes,
                heap,
                sort_end: None,
                plan_push_cursor: None,
                sort_sift_cursor: None,
                arrived: kept_arrived,
                key_bytes: 0,
                key_reader: TopKKeyReader::new(file),
                next_offset,
                free_slots: Vec::new(),
                pending_write: Some(PendingTopKWrite {
                    completion,
                    write_err,
                    entries,
                    count_new: 1,
                }),
                pending_read: None,
            }
        };
        self.pending_demote = Some(spilled);
        let res = self.poll_pending_demote();
        // Under `TempStore::Default`, if the first disk write fails (e.g. full `TMPDIR` / ENOSPC),
        // retry once on in-memory temp storage while `self.kept` is still intact.
        if res.is_err()
            && matches!(self.temp_store, crate::TempStore::Default)
            && self
                .temp_file
                .as_ref()
                .is_some_and(TempFile::is_disk_backed)
            && self.kept.is_some()
        {
            self.temp_file = Some(TempFile::with_temp_store(
                &self.io,
                crate::TempStore::Memory,
            )?);
            return self.demote_kept_to_spilled_top_k(incoming);
        }
        let _ = return_if_io!(res);
        Ok(IOResult::Done(()))
    }

    pub const fn has_more(&self) -> bool {
        self.current.is_some()
    }

    /// Amphora (PL.13, lane replan-5): the rows put in so far, whether kept, held or spilled.
    pub fn inserted(&self) -> u64 {
        self.inserted
    }

    /// Amphora (QT.5): the rows the last in-memory sort ordered and the nanoseconds it took, once.
    pub fn take_in_memory_sort(&mut self) -> Option<(usize, f64)> {
        self.in_memory_sort.take()
    }

    // We do the sorting here since this is what is called by the SorterSort instruction
    pub fn sort(&mut self) -> IOResultOr<()> {
        loop {
            match self.sort_state {
                SortState::Start => {
                    let _ = return_if_io!(self.poll_pending_demote());
                    let _ = return_if_io!(self.poll_pending_chunk_demote());
                    if let Some(spilled) = self.spilled_top_k.as_mut() {
                        let added = return_if_io!(spilled.poll_pending_write(
                            self.key_len,
                            &self.index_key_info,
                            &self.comparators,
                        ));
                        self.inserted = self.inserted.saturating_add(added);
                        return_if_io!(spilled.finish_sort_async(
                            self.key_len,
                            &self.index_key_info,
                            &self.comparators,
                        ));
                        self.sort_state = SortState::Next;
                    } else if let Some(kept) = self.kept.as_mut().filter(|k| k.limit > 0) {
                        // The kept rows in output order, last first: served from the end.
                        let mut rows = std::mem::take(&mut kept.heap).into_vec();
                        kept.bytes = 0;
                        rows.sort_unstable();
                        rows.reverse();
                        kept.sorted = rows;
                        self.sort_state = SortState::Next;
                    } else if self.chunks.is_empty() {
                        // Sort ascending then reverse - we pop from end so this gives ascending output.
                        // NOTE: We can't just sort descending because stable sort preserves insertion
                        // order for equal elements, and descending sort doesn't reverse equal elements.
                        // SAFETY: All pointers in records are valid (arena hasn't been reset).
                        let started = std::time::Instant::now();
                        self.records
                            .sort_by(|a, b| unsafe { a.as_ref().cmp(b.as_ref()) });
                        self.in_memory_sort =
                            Some((self.records.len(), started.elapsed().as_nanos() as f64));
                        self.records.reverse();
                        self.sort_state = SortState::Next;
                    } else {
                        self.sort_state = SortState::Flush;
                    }
                }
                SortState::Flush => {
                    self.sort_state = SortState::InitHeap;
                    if let Some(c) = self.flush()? {
                        io_yield_one!(c);
                    }
                }
                SortState::InitHeap => {
                    // Check for write errors before proceeding
                    if self.chunks.iter().any(|chunk| {
                        matches!(*chunk.io_state.read(), SortedChunkIOState::WriteError)
                    }) {
                        return Err(CompletionError::IOError(
                            std::io::ErrorKind::WriteZero,
                            "sorter write",
                        )
                        .into());
                    }
                    turso_assert!(
                        !self.chunks.iter().any(|chunk| {
                            matches!(*chunk.io_state.read(), SortedChunkIOState::WaitingForWrite)
                        }),
                        "chunks should been written"
                    );
                    return_if_io!(self.init_chunk_heap());
                    self.sort_state = SortState::Next;
                }
                SortState::Next => {
                    return_if_io!(self.next());
                    self.sort_state = SortState::Start;
                    return Ok(IOResult::Done(()));
                }
            }
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> IOResultOr<()> {
        if let Some(spilled) = self.spilled_top_k.as_mut() {
            return_if_io!(spilled.read_next_into(&mut self.current));
            return Ok(IOResult::Done(()));
        }
        if let Some(kept) = self.kept.as_mut().filter(|k| k.limit > 0) {
            self.current = kept.sorted.pop().map(|kept| kept.record.record);
            return Ok(IOResult::Done(()));
        }
        if self.chunks.is_empty() {
            match self.records.pop() {
                Some(ptr) => {
                    // SAFETY: ptr is valid - arena hasn't been reset yet.
                    let arena_record = unsafe { ptr.as_ref() };
                    let payload = arena_record.payload();

                    match &mut self.current {
                        Some(record) => {
                            record.invalidate();
                            record.start_serialization(payload)?;
                        }
                        None => {
                            self.current = Some(arena_record.to_immutable_record()?);
                        }
                    }

                    if self.records.is_empty() {
                        self.arena.reset();
                    }
                }
                None => self.current = None,
            }
        } else {
            if let Some(limit) = self.output_limit {
                if self.emitted >= limit {
                    self.current = None;
                    return Ok(IOResult::Done(()));
                }
            }
            // Serve from sorted chunk files
            match return_if_io!(self.next_from_chunk_heap()) {
                Some(boxed_record) => {
                    if let Some(ref error) = boxed_record.deserialization_error {
                        return Err(error.clone().into());
                    }
                    let payload = boxed_record.record.get_payload();
                    match &mut self.current {
                        Some(record) => {
                            record.invalidate();
                            record.start_serialization(payload)?;
                        }
                        None => {
                            self.current = Some(boxed_record.record);
                        }
                    }
                    self.emitted = self.emitted.saturating_add(1);
                }
                None => self.current = None,
            }
        }
        Ok(IOResult::Done(()))
    }

    pub const fn record(&self) -> Option<&ImmutableRecord> {
        self.current.as_ref()
    }

    /// Moves the current record out of the sorter, seeding the next
    /// [Sorter::next] call with `spent`'s allocation. This lets SorterData
    /// transfer records to a register with no allocation or payload copy.
    pub fn take_current(&mut self, spent: RecordBuf) -> Option<ImmutableRecord> {
        let current = self.current.take();
        if current.is_some() {
            self.current = Some(ImmutableRecord::from_buf(spent));
        }
        current
    }

    pub fn insert(&mut self, record: &ImmutableRecord) -> IOResultOr<()> {
        if return_if_io!(self.poll_pending_demote()) {
            return Ok(IOResult::Done(()));
        }
        let _ = return_if_io!(self.poll_pending_chunk_demote());
        if let Some(spilled) = self.spilled_top_k.as_mut() {
            let added = return_if_io!(spilled.offer(
                record,
                self.key_len,
                &self.index_key_info,
                &self.comparators,
            ));
            self.inserted = self.inserted.saturating_add(added);
            return Ok(IOResult::Done(()));
        }
        if let Some(kept) = self.kept.as_mut().filter(|k| k.limit > 0) {
            match kept.classify_insert(
                record,
                self.key_len,
                &self.index_key_info,
                &self.comparators,
                self.max_buffer_size,
            )? {
                KeptBudget::Reject => {
                    self.inserted = self.inserted.saturating_add(1);
                    return Ok(IOResult::Done(()));
                }
                KeptBudget::Fits => {
                    kept.insert_winner(
                        record,
                        self.key_len,
                        &self.index_key_info,
                        &self.comparators,
                    )?;
                    self.inserted = self.inserted.saturating_add(1);
                    return Ok(IOResult::Done(()));
                }
                KeptBudget::Exceeds => {
                    let spilled_meta = kept
                        .limit
                        .saturating_mul(std::mem::size_of::<SpilledTopKEntry>());
                    if spilled_meta <= self.max_buffer_size {
                        return self.demote_kept_to_spilled_top_k(record);
                    }
                    // When even the `limit`-entry metadata table exceeds `max_buffer_size`
                    // (e.g. `LIMIT 8192` under a 100 KiB budget), demote to budget-bounded
                    // chunked external merge sort with `output_limit` instead of over-allocating.
                    return_if_io!(self.demote_kept_to_chunks());
                }
            }
        }
        let payload_size = record.get_payload().len();
        loop {
            match self.insert_state {
                InsertState::Start => {
                    self.insert_state = InsertState::Insert;
                    // Track live payload + per-record arena/pointer metadata (NOT retained bump
                    // capacity, which persists across `arena.reset()`) so small rows still spill
                    // within `max_buffer_size` without fragmenting into 1-row chunks.
                    let per_record_meta = std::mem::size_of::<ArenaSortableRecord>()
                        .saturating_add(
                            self.key_len.saturating_mul(std::mem::size_of::<ValueRef>()),
                        )
                        .saturating_add(std::mem::size_of::<NonNull<ArenaSortableRecord>>());
                    let live_arena_bytes = self
                        .current_buffer_size
                        .saturating_add(self.records.len().saturating_mul(per_record_meta));
                    let exceeds = self.current_buffer_size.saturating_add(payload_size)
                        > self.max_buffer_size
                        || (!self.records.is_empty()
                            && live_arena_bytes
                                .saturating_add(payload_size)
                                .saturating_add(per_record_meta)
                                > self.max_buffer_size);
                    if exceeds {
                        if let Some(c) = self.flush()? {
                            if !c.succeeded() {
                                io_yield_one!(c);
                            }
                        }
                        // Check for write errors immediately after flush completes
                        if self.chunks.iter().any(|chunk| {
                            matches!(*chunk.io_state.read(), SortedChunkIOState::WriteError)
                        }) {
                            return Err(CompletionError::IOError(
                                std::io::ErrorKind::WriteZero,
                                "sorter write",
                            )
                            .into());
                        }
                    }
                }
                InsertState::Insert => {
                    let sortable_record = ArenaSortableRecord::new(
                        &self.arena,
                        record,
                        self.key_len,
                        &self.index_key_info,
                        &self.comparators,
                    )?;
                    let record_ref = self.arena.try_alloc(sortable_record)?;
                    // SAFETY: try_alloc returns a valid, aligned, non-null pointer.
                    self.records.try_push(NonNull::from(record_ref))?;
                    self.current_buffer_size += payload_size;
                    self.max_payload_size_in_buffer =
                        self.max_payload_size_in_buffer.max(payload_size);
                    self.insert_state = InsertState::Start;
                    self.inserted = self.inserted.saturating_add(1);
                    return Ok(IOResult::Done(()));
                }
            }
        }
    }

    fn init_chunk_heap(&mut self) -> IOResultOr<()> {
        match self.init_chunk_heap_state {
            InitChunkHeapState::Start => {
                turso_assert!(self.records.is_empty());
                self.records = vec![];
                self.arena = Bump::new();
                let per_chunk_budget = self
                    .max_buffer_size
                    .checked_div(self.chunks.len())
                    .unwrap_or(self.max_buffer_size);
                // Keep `min_chunk_read_buffer_size` only when all chunk buffers together fit
                // inside `max_buffer_size`; always honor each chunk's hard `min_buffer_size`
                // (`max_record + 9`) so every record decodes.
                let read_buffer_size = if self
                    .min_chunk_read_buffer_size
                    .saturating_mul(self.chunks.len())
                    <= self.max_buffer_size
                {
                    per_chunk_budget.max(self.min_chunk_read_buffer_size)
                } else {
                    per_chunk_budget
                };
                self.chunk_heap.try_reserve(self.chunks.len())?;
                for chunk in self.chunks.iter_mut() {
                    let target = read_buffer_size.max(chunk.min_buffer_size);
                    chunk.grow_buffer(target)?;
                }
                let mut group = CompletionGroup::new(|_| {});
                for chunk in self.chunks.iter_mut() {
                    if let Err(e) = chunk.read(Some(&mut group)) {
                        tracing::error!("Failed to read chunk: {e}");
                        group.cancel();
                        self.io.drain_completions(group.completions())?;
                        return Err(e.into());
                    }
                }
                self.init_chunk_heap_state = InitChunkHeapState::PushChunk { chunk_idx: 0 };
                let completion = group.build();
                io_yield_one!(completion);
            }
            InitChunkHeapState::PushChunk { mut chunk_idx } => {
                // Make sure all chunks read at least one record into their buffer.
                turso_assert!(
                    !self.chunks.iter().any(|chunk| matches!(
                        *chunk.io_state.read(),
                        SortedChunkIOState::WaitingForRead
                    )),
                    "chunks should have been read"
                );
                while chunk_idx < self.chunks.len() {
                    if let Some(completion) = self.push_to_chunk_heap(chunk_idx)? {
                        io_yield_one!(completion);
                    }
                    chunk_idx += 1;
                    self.init_chunk_heap_state = InitChunkHeapState::PushChunk { chunk_idx };
                }
                self.init_chunk_heap_state = InitChunkHeapState::Start;
                Ok(IOResult::Done(()))
            }
        }
    }

    /// Returns the next record from the chunk heap in sorted order.
    ///
    /// The heap contains at most one record per chunk. When we pop a record, we try to refill
    /// from that chunk. If IO is needed, we store it in `pending_completion` and wait for it
    /// on the next call before popping again - this ensures all non-exhausted chunks have
    /// a record in the heap before we decide which is smallest.
    fn next_from_chunk_heap(&mut self) -> IOResultOr<Option<Box<BoxedSortableRecord>>> {
        // If there is a pending IO, we must wait for it before popping from the heap,
        // otherwise we might return records out of order.
        while let Some((completion, chunk_idx)) = self.pending_completion.take() {
            if !completion.succeeded() {
                // IO not complete - put it back and yield
                self.pending_completion = Some((completion.clone(), chunk_idx));
                return Ok(IOResult::IO(IOCompletions(completion)));
            }
            // IO completed - push result to heap and retry
            if let Some(c) = self.push_to_chunk_heap(chunk_idx)? {
                self.pending_completion = Some((c, chunk_idx));
            }
        }

        // No pending IO - safe to pop from heap
        if let Some((next_record, Reverse(chunk_idx))) = self.chunk_heap.pop() {
            if let Some(c) = self.push_to_chunk_heap(chunk_idx)? {
                self.pending_completion = Some((c, chunk_idx));
            }
            return Ok(IOResult::Done(Some(next_record.0)));
        }

        // Heap empty and no pending IO - sorter exhausted
        Ok(IOResult::Done(None))
    }

    /// Pushes the next record of chunk `chunk_idx` onto the heap. If the
    /// chunk needs a read first, the read's completion is returned.
    fn push_to_chunk_heap(&mut self, chunk_idx: usize) -> Result<Option<Completion>> {
        let chunk = &mut self.chunks[chunk_idx];

        match chunk.next()? {
            ChunkNextResult::Done(Some(record)) => {
                self.chunk_heap.try_push((
                    Reverse(Box::new(BoxedSortableRecord::new(
                        record,
                        self.key_len,
                        self.index_key_info.clone(),
                        self.comparators.clone(),
                    )?)),
                    Reverse(chunk_idx),
                ))?;
                Ok(None)
            }
            ChunkNextResult::Done(None) => Ok(None),
            ChunkNextResult::IO(io) => Ok(Some(io)),
        }
    }

    fn flush(&mut self) -> Result<Option<Completion>> {
        if self.records.is_empty() {
            // Dummy completion to not complicate logic handling
            return Ok(None);
        }

        // SAFETY: All pointers are valid (arena not reset).
        self.records
            .sort_by(|a, b| unsafe { a.as_ref().cmp(b.as_ref()) });

        let chunk_file = self.open_temp_file()?;

        // Hard minimum buffer size required to decode the largest record in this chunk plus its
        // size varint; `init_chunk_heap` scales up toward `min_chunk_read_buffer_size` / budget.
        let chunk_buffer_size = self.max_payload_size_in_buffer.saturating_add(9);

        let mut chunk_size = 0;
        // Pre-compute varint lengths for record sizes to determine the total buffer size.
        // SAFETY: All pointers are valid because they are allocated in the arena,
        // and the arena hasn't been reset.
        let mut record_size_lengths = Vec::try_with_capacity_ext(self.records.len())?;
        for ptr in self.records.iter() {
            let record_size = unsafe { ptr.as_ref().payload().len() };
            let size_len = varint_len(record_size as u64);
            // Enough space was preallocated to `push` instead of `try_push`
            record_size_lengths.push(size_len);
            chunk_size += size_len + record_size;
        }

        let mut chunk = SortedChunk::new(chunk_file, self.next_chunk_offset, chunk_buffer_size)?;
        let c = chunk.write(&self.records, record_size_lengths, chunk_size)?;
        self.chunks.try_push(chunk)?;

        self.records.clear();
        if self.arena.allocated_bytes() > self.max_buffer_size {
            self.arena = Bump::new();
        } else {
            self.arena.reset();
        }

        self.current_buffer_size = 0;
        self.max_payload_size_in_buffer = 0;
        // increase offset start for next chunk
        self.next_chunk_offset += chunk_size;

        Ok(Some(c))
    }
}

#[derive(Debug, Clone, Copy)]
enum NextState {
    Start,
    Finish,
}

/// A sorted chunk represents a portion of sorted data that has been written to disk
/// during external merge sort. When the in-memory buffer fills up, records are sorted
/// and flushed to a chunk file. During the merge phase, chunks are read back and merged
/// using a heap to produce the final sorted output.
///
/// # Buffer management
///
/// The chunk uses a read buffer (`buffer`) to read data from disk. The buffer
/// has two relevant sizes:
/// - `buffer.len()` (capacity): The total allocated size of the buffer. It is set at
///   creation and can only grow with [SortedChunk::grow_buffer] before the first read.
/// - `buffer_len`: The amount of valid data currently in the buffer (0 to capacity)
///
/// The difference `buffer.len() - buffer_len` is the free space available for reading
/// more data from disk.
///
/// # Reading progress
///
/// - `chunk_size`: Total bytes of this chunk on disk (set when chunk is written)
/// - `total_bytes_read`: Cumulative bytes read from disk so far (0 to chunk_size)
///
/// The difference `chunk_size - total_bytes_read` is the remaining data on disk that
/// hasn't been read yet. When `total_bytes_read == chunk_size`, we've read all data.
///
/// # Record parsing
///
/// Data flows: disk -> buffer -> records -> caller
///
/// 1. `read()` fills `buffer` from disk, updates `total_bytes_read`
/// 2. `next()` parses records from `buffer` into `records` vec, updates `buffer_len`
/// 3. `next()` returns records one at a time from `records`
///
/// Incomplete records at the end of the buffer are kept (buffer compacted) until
/// more data is read to complete them.
struct SortedChunk {
    /// The file containing the chunk data.
    file: Arc<dyn File>,
    /// Byte offset where this chunk starts in the file.
    start_offset: u64,
    /// Total size of this chunk in bytes (set during write, used to detect EOF during read).
    chunk_size: usize,
    /// Minimum read buffer size required to decode the largest record in this chunk (allocated
    /// lazily in `init_chunk_heap`, not at `flush()` time).
    min_buffer_size: usize,
    /// Buffer for reading data from disk. Allocated lazily when merging begins and released on
    /// exhaustion (`ReadEOF`).
    buffer: Arc<RwLock<Vec<u8>>>,
    /// Amount of valid (unparsed) data in `buffer`, from index 0 to buffer_len.
    /// This is separate from buffer.len() because we reuse the same allocation.
    buffer_len: Arc<atomic::AtomicUsize>,
    /// Current read cursor inside `buffer[0..buffer_len]` so records stream one at a time without
    /// materializing an entire `Vec<ImmutableRecord>` per chunk buffer.
    buffer_pos: usize,
    /// Single decoded record staged across an async prefetch read (`NextState::Finish`).
    staged: Option<ImmutableRecord>,
    /// Current async IO state (None, WaitingForRead, ReadComplete, ReadEOF, etc).
    io_state: Arc<RwLock<SortedChunkIOState>>,
    /// Cumulative bytes read from disk. When this equals `chunk_size`, we've read everything.
    total_bytes_read: Arc<atomic::AtomicUsize>,
    /// State machine for the `next()` method.
    next_state: NextState,
}

enum ChunkNextResult {
    Done(Option<ImmutableRecord>),
    IO(Completion),
}

impl SortedChunk {
    fn new(file: Arc<dyn File>, start_offset: usize, buffer_size: usize) -> Result<Self> {
        Ok(Self {
            file,
            start_offset: start_offset as u64,
            chunk_size: 0,
            min_buffer_size: buffer_size,
            buffer: Arc::new(RwLock::new(Vec::new())),
            buffer_len: Arc::new(atomic::AtomicUsize::new(0)),
            buffer_pos: 0,
            staged: None,
            io_state: Arc::new(RwLock::new(SortedChunkIOState::None)),
            total_bytes_read: Arc::new(atomic::AtomicUsize::new(0)),
            next_state: NextState::Start,
        })
    }

    fn buffer_len(&self) -> usize {
        self.buffer_len.load(atomic::Ordering::SeqCst)
    }

    /// Grows the read buffer to `size` bytes, but never past the size of the
    /// chunk. Must be called before the first read.
    fn grow_buffer(&mut self, size: usize) -> Result<()> {
        turso_assert_eq!(self.total_bytes_read.load(atomic::Ordering::SeqCst), 0);
        let size = size.min(self.chunk_size);
        let mut buffer = self.buffer.write();
        if size > buffer.len() {
            let additional = size - buffer.len();
            buffer.try_reserve_exact(additional)?;
            buffer.resize(size, 0);
        }
        Ok(())
    }

    fn set_buffer_len(&self, len: usize) {
        self.buffer_len.store(len, atomic::Ordering::SeqCst);
    }

    /// Checks whether `buffer[pos..len]` holds another complete `(varint, payload)` record.
    fn has_complete_record(buffer: &[u8], pos: usize, len: usize) -> bool {
        let Some(slice) = buffer.get(pos..len) else {
            return false;
        };
        if slice.is_empty() {
            return false;
        }
        let Ok((record_size, bytes_read)) = read_varint(slice) else {
            return false;
        };
        let Some(rec_len) = usize::try_from(record_size).ok() else {
            return false;
        };
        rec_len <= len.saturating_sub(pos.saturating_add(bytes_read))
    }

    /// Returns the next record from this chunk, or None if exhausted.
    ///
    /// Streams one record at a time directly from `self.buffer` using `self.buffer_pos` (holding
    /// at most one `staged` record across an async prefetch) so merging many chunks stays strictly
    /// within the sorter's configured byte budget.
    fn next(&mut self) -> Result<ChunkNextResult> {
        loop {
            match self.next_state {
                NextState::Start => {
                    let buffer_len = self.buffer_len();
                    let is_eof = *self.io_state.read() == SortedChunkIOState::ReadEOF;
                    if self.staged.is_none() && self.buffer_pos >= buffer_len {
                        if is_eof
                            || self.total_bytes_read.load(atomic::Ordering::SeqCst)
                                >= self.chunk_size
                        {
                            // Release the read buffer as soon as the chunk is exhausted.
                            *self.buffer.write() = Vec::new();
                            self.buffer_pos = 0;
                            self.set_buffer_len(0);
                            return Ok(ChunkNextResult::Done(None));
                        }
                        self.buffer_pos = 0;
                        self.set_buffer_len(0);
                        if let Some(c) = self.read(None)? {
                            if !c.succeeded() {
                                return Ok(ChunkNextResult::IO(c));
                            }
                        }
                        continue;
                    }

                    if self.staged.is_none() {
                        let (decoded, need_prefetch) = {
                            let mut buffer_ref = self.buffer.write();
                            let buffer = buffer_ref.as_mut_slice();
                            let pos = self.buffer_pos;
                            let unread = buffer.get(pos..buffer_len).ok_or_else(|| {
                                LimboError::InternalError("sorter chunk unread slice".into())
                            })?;
                            let parse = match read_varint(unread) {
                                Ok((record_size, bytes_read)) => {
                                    let rec_len = usize::try_from(record_size)
                                        .map_err(|_| LimboError::IntegerOverflow)?;
                                    if rec_len
                                        > buffer_len.saturating_sub(pos.saturating_add(bytes_read))
                                    {
                                        if is_eof {
                                            crate::bail_corrupt_error!("Incomplete record");
                                        }
                                        None
                                    } else {
                                        let payload_start = pos.saturating_add(bytes_read);
                                        let payload_end = payload_start.saturating_add(rec_len);
                                        let payload_slice = buffer
                                            .get(payload_start..payload_end)
                                            .ok_or_else(|| {
                                                LimboError::InternalError(
                                                    "sorter chunk payload slice".into(),
                                                )
                                            })?;
                                        let mut record = ImmutableRecord::new(rec_len)?;
                                        record.start_serialization(payload_slice)?;
                                        Some((record, payload_end))
                                    }
                                }
                                Err(LimboError::Corrupt(_)) if !is_eof => None,
                                Err(e) => return Err(e),
                            };
                            match parse {
                                Some((record, next_pos)) => {
                                    let more_in_buf =
                                        Self::has_complete_record(buffer, next_pos, buffer_len);
                                    if !more_in_buf {
                                        // Compact any trailing partial record bytes so the
                                        // prefetch read fills the freed buffer space.
                                        if next_pos < buffer_len {
                                            buffer.copy_within(next_pos..buffer_len, 0);
                                            self.set_buffer_len(
                                                buffer_len.saturating_sub(next_pos),
                                            );
                                        } else {
                                            self.set_buffer_len(0);
                                        }
                                        self.buffer_pos = 0;
                                    } else {
                                        self.buffer_pos = next_pos;
                                    }
                                    (Some(record), !more_in_buf)
                                }
                                None => {
                                    // Incomplete record at end of buffer: compact and read more.
                                    if pos > 0 && pos < buffer_len {
                                        buffer.copy_within(pos..buffer_len, 0);
                                        self.set_buffer_len(buffer_len.saturating_sub(pos));
                                        self.buffer_pos = 0;
                                    }
                                    (None, true)
                                }
                            }
                        };

                        let Some(record) = decoded else {
                            if let Some(c) = self.read(None)? {
                                if !c.succeeded() {
                                    return Ok(ChunkNextResult::IO(c));
                                }
                            }
                            continue;
                        };
                        self.staged = Some(record);
                        self.next_state = NextState::Finish;
                        // Prefetch when the buffer no longer holds another complete record so the
                        // next pop overlaps disk I/O without buffering extra `ImmutableRecord`s.
                        if need_prefetch && !is_eof {
                            if let Some(c) = self.read(None)? {
                                if !c.succeeded() {
                                    return Ok(ChunkNextResult::IO(c));
                                }
                            }
                        }
                    } else {
                        self.next_state = NextState::Finish;
                    }
                }
                NextState::Finish => {
                    self.next_state = NextState::Start;
                    return Ok(ChunkNextResult::Done(self.staged.take()));
                }
            }
        }
    }

    /// Issues an async read to fill the buffer with more data from the chunk file.
    ///
    /// Reads up to `min(free_buffer_space, remaining_chunk_bytes)` bytes. Returns `None`
    /// if there's no room in the buffer or no data left to read (no IO issued).
    ///
    /// On completion, appends data to `buffer` and updates `buffer_len` and `total_bytes_read`.
    /// Reads more of the chunk file into the buffer. The read is added to
    /// `group`, when given, before it is submitted.
    fn read(&mut self, group: Option<&mut CompletionGroup>) -> Result<Option<Completion>> {
        let free_buffer_space = self.buffer.read().len() - self.buffer_len();
        let remaining_chunk_bytes =
            self.chunk_size - self.total_bytes_read.load(atomic::Ordering::SeqCst);
        let read_buffer_size = free_buffer_space.min(remaining_chunk_bytes);

        // If there's no room in the buffer or nothing left to read, skip the read.
        if read_buffer_size == 0 {
            if remaining_chunk_bytes == 0 {
                // No more data in the chunk file.
                *self.io_state.write() = SortedChunkIOState::ReadEOF;
            }
            return Ok(None);
        }

        *self.io_state.write() = SortedChunkIOState::WaitingForRead;

        let read_buffer = Buffer::new_temporary(read_buffer_size);
        let read_buffer_ref = Arc::new(read_buffer);

        let chunk_io_state_copy = self.io_state.clone();
        let stored_buffer_copy = self.buffer.clone();
        let stored_buffer_len_copy = self.buffer_len.clone();
        let total_bytes_read_copy = self.total_bytes_read.clone();
        let read_complete = Box::new(move |res: Result<(Arc<Buffer>, i32), CompletionError>| {
            let Ok((buf, bytes_read)) = res else {
                return None;
            };
            let read_buf = buf.as_slice();

            let bytes_read = bytes_read as usize;
            if bytes_read == 0 {
                *chunk_io_state_copy.write() = SortedChunkIOState::ReadEOF;
                return None;
            }
            *chunk_io_state_copy.write() = SortedChunkIOState::ReadComplete;

            let mut stored_buf_ref = stored_buffer_copy.write();
            let stored_buf = stored_buf_ref.as_mut_slice();
            let mut stored_buf_len = stored_buffer_len_copy.load(atomic::Ordering::SeqCst);

            stored_buf[stored_buf_len..stored_buf_len + bytes_read]
                .copy_from_slice(&read_buf[..bytes_read]);
            stored_buf_len += bytes_read;

            stored_buffer_len_copy.store(stored_buf_len, atomic::Ordering::SeqCst);
            total_bytes_read_copy.fetch_add(bytes_read, atomic::Ordering::SeqCst);
            None
        });

        let c = Completion::new_read(read_buffer_ref, read_complete);
        if let Some(group) = group {
            group.add(&c);
        }
        let c = self.file.pread(
            self.start_offset + self.total_bytes_read.load(atomic::Ordering::SeqCst) as u64,
            c,
        )?;
        Ok(Some(c))
    }

    fn write(
        &mut self,
        records: &[NonNull<ArenaSortableRecord>],
        record_size_lengths: Vec<usize>,
        chunk_size: usize,
    ) -> Result<Completion> {
        turso_assert_eq!(*self.io_state.read(), SortedChunkIOState::None);
        *self.io_state.write() = SortedChunkIOState::WaitingForWrite;
        self.chunk_size = chunk_size;

        let buffer = Buffer::new_temporary(self.chunk_size);

        let mut buf_pos = 0;
        let buf = buffer.as_mut_slice();
        for (ptr, size_len) in records.iter().zip(record_size_lengths) {
            // SAFETY: All pointers are valid (arena not reset).
            let payload = unsafe { ptr.as_ref().payload() };
            // Write the record size varint.
            write_varint(&mut buf[buf_pos..buf_pos + size_len], payload.len() as u64);
            buf_pos += size_len;
            // Write the record payload.
            buf[buf_pos..buf_pos + payload.len()].copy_from_slice(payload);
            buf_pos += payload.len();
        }

        let buffer_ref = Arc::new(buffer);

        let buffer_ref_copy = buffer_ref.clone();
        let chunk_io_state_copy = self.io_state.clone();
        let write_complete = Box::new(move |res: Result<i32, CompletionError>| {
            let Ok(bytes_written) = res else {
                *chunk_io_state_copy.write() = SortedChunkIOState::WriteError;
                return;
            };
            let buf_len = buffer_ref_copy.len();
            if bytes_written < buf_len as i32 {
                tracing::error!("wrote({bytes_written}) less than expected({buf_len})");
                *chunk_io_state_copy.write() = SortedChunkIOState::WriteError;
            } else {
                *chunk_io_state_copy.write() = SortedChunkIOState::WriteComplete;
            }
        });

        let c = Completion::new_write(write_complete);
        let c = self.file.pwrite(self.start_offset, buffer_ref, c)?;
        Ok(c)
    }

    fn write_kept_records(
        &mut self,
        records: &[&KeptRecord],
        record_size_lengths: &[usize],
        chunk_size: usize,
    ) -> Result<Completion> {
        if *self.io_state.read() != SortedChunkIOState::None {
            return Err(LimboError::InternalError(
                "chunk write started while IO state was not None".into(),
            ));
        }
        *self.io_state.write() = SortedChunkIOState::WaitingForWrite;
        self.chunk_size = chunk_size;

        let buffer = Buffer::new_temporary(self.chunk_size);
        let mut buf_pos = 0usize;
        let buf = buffer.as_mut_slice();
        for (row, &size_len) in records.iter().zip(record_size_lengths.iter()) {
            let payload = row.record.record.get_payload();
            let varint_end = buf_pos.saturating_add(size_len);
            let varint_slice = buf
                .get_mut(buf_pos..varint_end)
                .ok_or_else(|| LimboError::InternalError("sorter chunk varint slice".into()))?;
            write_varint(varint_slice, payload.len() as u64);
            buf_pos = varint_end;
            let payload_end = buf_pos.saturating_add(payload.len());
            let payload_slice = buf
                .get_mut(buf_pos..payload_end)
                .ok_or_else(|| LimboError::InternalError("sorter chunk payload slice".into()))?;
            payload_slice.copy_from_slice(payload);
            buf_pos = payload_end;
        }

        let buffer_ref = Arc::new(buffer);
        let buffer_ref_copy = buffer_ref.clone();
        let chunk_io_state_copy = self.io_state.clone();
        let write_complete = Box::new(move |res: Result<i32, CompletionError>| {
            let Ok(bytes_written) = res else {
                *chunk_io_state_copy.write() = SortedChunkIOState::WriteError;
                return;
            };
            let buf_len = buffer_ref_copy.len();
            if i32::try_from(buf_len)
                .ok()
                .is_none_or(|expected| bytes_written < expected)
            {
                tracing::error!("wrote({bytes_written}) less than expected({buf_len})");
                *chunk_io_state_copy.write() = SortedChunkIOState::WriteError;
            } else {
                *chunk_io_state_copy.write() = SortedChunkIOState::WriteComplete;
            }
        });
        let c = Completion::new_write(write_complete);
        let c = self.file.pwrite(self.start_offset, buffer_ref, c)?;
        Ok(c)
    }
}

/// Record for in-memory sorting. All data lives in the arena, so no Drop is needed.
struct ArenaSortableRecord {
    /// Payload bytes in arena. Using NonNull avoids lifetime issues with
    /// self-referential struct (key_values points into this payload).
    payload: NonNull<[u8]>,
    /// Pre-computed key values in arena. Points into `payload`.
    key_values: NonNull<[ValueRef<'static>]>,
    /// Shared KeyInfo owned by Sorter. Avoids Rc refcount overhead that would
    /// leak when arena.reset() skips Drop.
    index_key_info: NonNull<[KeyInfo]>,
    /// Shared comparators owned by Sorter. Same safety model as index_key_info.
    comparators: NonNull<[Option<SortComparator>]>,
    /// Order-preserving prefix of the first key column; see [normalized_first_key].
    norm_key: u64,
    /// True when equal `norm_key`s prove the full keys are equal.
    norm_decisive: bool,
}

impl ArenaSortableRecord {
    fn new(
        arena: &Bump,
        record: &ImmutableRecord,
        key_len: usize,
        index_key_info: &[KeyInfo],
        comparators: &[Option<SortComparator>],
    ) -> Result<Self> {
        let payload = arena.try_alloc_slice_copy(record.get_payload())?;

        let mut payload_iter = ValueIterator::new(payload)?;

        let mut key_values = bumpalo::collections::Vec::new_in(arena);
        key_values.try_reserve(key_len)?;
        for _ in 0..key_len {
            let value = match payload_iter.next() {
                Some(Ok(v)) => v,
                Some(Err(e)) => return Err(e),
                None => crate::bail_corrupt_error!("Not enough columns in record"),
            };
            // SAFETY: value borrows from payload which is in the arena and outlives this struct.
            let value: ValueRef<'static> = unsafe { std::mem::transmute(value) };
            key_values.push(value);
        }

        let key_values = key_values.into_bump_slice();
        let (norm_key, norm_decisive) =
            normalized_first_key(key_values, index_key_info, comparators);
        Ok(Self {
            payload: NonNull::from(payload),
            key_values: NonNull::from(key_values),
            index_key_info: NonNull::from(index_key_info),
            comparators: NonNull::from(comparators),
            norm_key,
            norm_decisive,
        })
    }

    #[inline]
    const fn key_values(&self) -> &[ValueRef<'static>] {
        // SAFETY: valid from construction, arena not reset
        unsafe { self.key_values.as_ref() }
    }

    #[inline]
    const fn payload(&self) -> &[u8] {
        // SAFETY: valid from construction, arena not reset
        unsafe { self.payload.as_ref() }
    }

    /// Create an ImmutableRecord by copying payload bytes out of the arena.
    fn to_immutable_record(&self) -> Result<ImmutableRecord> {
        let payload = self.payload();
        let mut record = ImmutableRecord::new(payload.len())?;
        record.start_serialization(payload)?;
        Ok(record)
    }
}

impl ArenaSortableRecord {
    /// Full key comparison; only reached when the normalized keys tie.
    fn full_cmp(&self, other: &Self) -> Ordering {
        let self_values = self.key_values();
        let other_values = other.key_values();
        // SAFETY: index_key_info and comparators point to Sorter-owned data that outlives all records.
        let index_key_info = unsafe { self.index_key_info.as_ref() };
        let comparators = unsafe { self.comparators.as_ref() };

        for (i, ((&self_val, &other_val), key_info)) in self_values
            .iter()
            .zip(other_values.iter())
            .zip(index_key_info.iter())
            .enumerate()
        {
            let cmp = if let Some(Some(comparator)) = comparators.get(i) {
                let base =
                    comparator(&self_val, &other_val).expect("Memory allocation failed here");
                cmp_with_sort(base, &self_val, &other_val, key_info)
            } else {
                cmp_in_column(&self_val, &other_val, key_info)
            };
            if cmp != Ordering::Equal {
                return cmp;
            }
        }

        Ordering::Equal
    }
}

impl Ord for ArenaSortableRecord {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        match self.norm_key.cmp(&other.norm_key) {
            Ordering::Equal if self.norm_decisive && other.norm_decisive => Ordering::Equal,
            Ordering::Equal => self.full_cmp(other),
            ord => ord,
        }
    }
}

impl PartialOrd for ArenaSortableRecord {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for ArenaSortableRecord {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for ArenaSortableRecord {}

/// Heap-allocated record for external merge sort. Used when records are read
/// back from chunk files. Normal Drop semantics apply.
struct BoxedSortableRecord {
    record: ImmutableRecord,
    key_values: smallvec::SmallVec<[ValueRef<'static>; 4]>,
    index_key_info: Rc<Vec<KeyInfo>>,
    comparators: Rc<Vec<Option<SortComparator>>>,
    deserialization_error: Option<LimboError>,
    /// Order-preserving prefix of the first key column; see [normalized_first_key].
    norm_key: u64,
    /// True when equal `norm_key`s prove the full keys are equal.
    norm_decisive: bool,
}

impl BoxedSortableRecord {
    fn new(
        record: ImmutableRecord,
        key_len: usize,
        index_key_info: Rc<Vec<KeyInfo>>,
        comparators: Rc<Vec<Option<SortComparator>>>,
    ) -> Result<Self> {
        let mut value_iterator = record.iter()?;
        let mut key_values: smallvec::SmallVec<[ValueRef<'static>; 4]> = smallvec::SmallVec::new();
        if key_len > 4 {
            key_values
                .try_reserve(key_len)
                .map_err(|_| LimboError::OutOfMemory)?;
        }
        let mut deserialization_error = None;

        for _ in 0..key_len {
            match value_iterator.next() {
                Some(Ok(value)) => {
                    // SAFETY: value points into record which lives as long as this struct
                    let value: ValueRef<'static> = unsafe { std::mem::transmute(value) };
                    key_values.push(value);
                }
                Some(Err(err)) => {
                    mark_unlikely();
                    deserialization_error = Some(err);
                    break;
                }
                None => {
                    mark_unlikely();
                    deserialization_error = Some(LimboError::Corrupt(
                        "Not enough columns in record".to_string(),
                    ));
                    break;
                }
            }
        }

        let (norm_key, norm_decisive) =
            normalized_first_key(&key_values, &index_key_info, &comparators);
        Ok(Self {
            record,
            key_values,
            index_key_info,
            comparators,
            deserialization_error,
            norm_key,
            norm_decisive,
        })
    }
}

impl BoxedSortableRecord {
    /// Full key comparison; only reached when the normalized keys tie.
    fn full_cmp(&self, other: &Self) -> Ordering {
        boxed_full_cmp(
            &self.key_values,
            &other.key_values,
            &self.index_key_info,
            &self.comparators,
        )
    }
}

/// The boxed records' full key comparison, of two keys' values.
fn boxed_full_cmp(
    self_values: &[ValueRef],
    other_values: &[ValueRef],
    index_key_info: &[KeyInfo],
    comparators: &[Option<SortComparator>],
) -> Ordering {
    for (i, ((&self_val, &other_val), key_info)) in self_values
        .iter()
        .zip(other_values.iter())
        .zip(index_key_info.iter())
        .enumerate()
    {
        let cmp = if let Some(Some(comparator)) = comparators.get(i) {
            comparator(&self_val, &other_val).expect("Memory allocation failed here")
        } else {
            match (self_val, other_val) {
                (ValueRef::Text(left), ValueRef::Text(right)) => {
                    key_info.collation.compare_strings(&left, &right)
                }
                _ => self_val.partial_cmp(&other_val).unwrap_or(Ordering::Equal),
            }
        };
        let cmp = cmp_with_sort(cmp, &self_val, &other_val, key_info);
        if cmp != Ordering::Equal {
            return cmp;
        }
    }
    Ordering::Equal
}

impl Ord for BoxedSortableRecord {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        if self.deserialization_error.is_some() || other.deserialization_error.is_some() {
            return Ordering::Equal;
        }
        match self.norm_key.cmp(&other.norm_key) {
            Ordering::Equal if self.norm_decisive && other.norm_decisive => Ordering::Equal,
            Ordering::Equal => self.full_cmp(other),
            ord => ord,
        }
    }
}

impl PartialOrd for BoxedSortableRecord {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for BoxedSortableRecord {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for BoxedSortableRecord {}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum SortedChunkIOState {
    WaitingForRead,
    ReadComplete,
    WaitingForWrite,
    WriteComplete,
    WriteError,
    ReadEOF,
    None,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::translate::collate::CollationSeq;
    use crate::types::{ImmutableRecord, Value, ValueRef, ValueType};
    use crate::util::IOExt;
    use crate::PlatformIO;
    use rand_chacha::{
        rand_core::{RngCore, SeedableRng},
        ChaCha8Rng,
    };

    fn get_seed() -> u64 {
        std::env::var("SEED").map_or_else(
            |_| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis()
            },
            |v| {
                v.parse()
                    .expect("Failed to parse SEED environment variable as u64")
            },
        ) as u64
    }

    /// Do: a sorter told to keep 5 rows and an unbounded one, each offered the same 1,000 rows whose
    /// descending key takes 50 values (twenty rows to a key), then sorted. Expect: the bounded one
    /// never holds more than 5, and serves exactly the unbounded one's first 5, ties in arrival
    /// order (docs/design/executor.md, decision 5).
    #[test]
    fn a_sorter_told_a_limit_keeps_only_that_many_and_serves_what_the_full_sort_begins_with() {
        let io = Arc::new(PlatformIO::new().unwrap());
        let new_sorter = || {
            Sorter::new(
                &[SortOrder::Desc],
                try_vec![CollationSeq::Binary].unwrap(),
                try_vec![None].unwrap(),
                try_vec![None].unwrap(),
                1 << 20,
                4096,
                io.clone(),
                crate::TempStore::Default,
            )
            .unwrap()
        };
        let (mut bounded, mut full) = (new_sorter(), new_sorter());
        bounded.limit_to(Some(5));
        full.limit_to(None);
        let mut x = 7u64;
        for i in 0..1000i64 {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let key = ((x >> 33) % 50) as i64;
            let values = [Value::from_i64(key), Value::from_i64(i)];
            let record = ImmutableRecord::from_values(&values, values.len()).unwrap();
            io.block(|| bounded.insert(&record)).unwrap();
            io.block(|| full.insert(&record)).unwrap();
            assert!(bounded.kept.as_ref().unwrap().heap.len() <= 5);
        }
        io.block(|| bounded.sort()).unwrap();
        io.block(|| full.sort()).unwrap();
        for _ in 0..5 {
            let want = full.record().unwrap().get_values().unwrap();
            let got = bounded.record().unwrap().get_values().unwrap();
            assert_eq!(got, want);
            io.block(|| bounded.next()).unwrap();
            io.block(|| full.next()).unwrap();
        }
        assert!(!bounded.has_more());
        assert!(full.has_more());
    }

    #[test]
    fn fuzz_normalized_key_invariant() {
        use crate::types::AsValueRef;
        use amphora_engine_parser::ast::NullsOrder;
        let seed = get_seed();
        let mut rng = ChaCha8Rng::seed_from_u64(seed);

        // Values chosen to stress every branch: type boundaries, f64 exactness
        // limits, shared prefixes, embedded NULs, lengths straddling 7 bytes.
        let gen_value = |rng: &mut ChaCha8Rng| -> Value {
            match rng.next_u64() % 10 {
                0 => Value::Null,
                1 => Value::from_i64(rng.next_u64() as i64),
                2 => Value::from_i64((rng.next_u64() % 100) as i64 - 50),
                3 => {
                    // Integers around the 2^49..2^53 exactness boundaries.
                    let base = 1i64 << (48 + rng.next_u64() % 8);
                    Value::from_i64(base + (rng.next_u64() % 5) as i64 - 2)
                }
                4 => {
                    let numerator = rng.next_u64() as f64;
                    let denominator = (rng.next_u64() as f64).abs().max(1.0);
                    Value::from_f64(numerator / denominator)
                }
                5 => Value::from_f64(if rng.next_u64().is_multiple_of(2) {
                    0.0
                } else {
                    -0.0
                }),
                6..=8 => {
                    let alphabet = *b"ab\0";
                    let len = (rng.next_u64() % 10) as usize;
                    let s: String = (0..len)
                        .map(|_| alphabet[(rng.next_u64() % 3) as usize] as char)
                        .collect();
                    Value::build_text(s)
                }
                _ => {
                    let len = (rng.next_u64() % 10) as usize;
                    let mut blob = try_vec![0u8; len].unwrap();
                    rng.fill_bytes(&mut blob);
                    Value::Blob(blob)
                }
            }
        };

        let gen_key = |rng: &mut ChaCha8Rng| KeyInfo {
            sort_order: if rng.next_u64().is_multiple_of(2) {
                SortOrder::Asc
            } else {
                SortOrder::Desc
            },
            collation: CollationSeq::Binary,
            nulls_order: match rng.next_u64() % 3 {
                0 => None,
                1 => Some(NullsOrder::First),
                _ => Some(NullsOrder::Last),
            },
        };

        for _ in 0..200_000 {
            // Half the iterations use a two-column key so the multi-column path
            // is covered: the normalized key still encodes only the first
            // column, and `decisive` must never be set (else equal first
            // columns would wrongly short-circuit past the second column).
            let ncols = 1 + (rng.next_u64() % 2) as usize;
            // Fixed-size arrays sliced to `ncols`: the crate's `Vec` alias is
            // allocator-parameterized under the nightly cfg and has no
            // `FromIterator`, so `collect()` into it would not compile.
            let va = [gen_value(&mut rng), gen_value(&mut rng)];
            let vb = [gen_value(&mut rng), gen_value(&mut rng)];
            let keys = [gen_key(&mut rng), gen_key(&mut rng)];
            let comparators: [Option<SortComparator>; 2] = [None, None];
            let ra = [va[0].as_value_ref(), va[1].as_value_ref()];
            let rb = [vb[0].as_value_ref(), vb[1].as_value_ref()];

            let (norm_a, dec_a) =
                normalized_first_key(&ra[..ncols], &keys[..ncols], &comparators[..ncols]);
            let (norm_b, dec_b) =
                normalized_first_key(&rb[..ncols], &keys[..ncols], &comparators[..ncols]);
            // The normalized key only ever reflects the first column.
            let a = &ra[0];
            let b = &rb[0];
            let key = &keys[0];
            let reference = cmp_in_column(a, b, key);

            if ncols > 1 {
                assert!(
                    !dec_a && !dec_b,
                    "multi-column keys must never be decisive: {va:?} vs {vb:?} keys {keys:?}"
                );
            }
            match norm_a.cmp(&norm_b) {
                Ordering::Equal => {
                    if dec_a && dec_b {
                        assert_eq!(
                            reference,
                            Ordering::Equal,
                            "decisive equal norms must mean equal keys: {va:?} vs {vb:?} keys {keys:?}"
                        );
                    }
                }
                ord => {
                    // A strict normalized order must match the first-column
                    // reference exactly: it may never contradict the reference,
                    // and it may never separate keys the reference deems equal
                    // (that would split GROUP BY groups or skip a later column).
                    assert_eq!(
                        ord, reference,
                        "strict norm order must match reference: {va:?} vs {vb:?} keys {keys:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn fuzz_external_sort() {
        let seed = get_seed();
        let mut rng = ChaCha8Rng::seed_from_u64(seed);

        let io = Arc::new(PlatformIO::new().unwrap());

        let attempts = 8;
        for _ in 0..attempts {
            let mut sorter = Sorter::new(
                &[SortOrder::Asc],
                try_vec![CollationSeq::Binary].unwrap(),
                try_vec![None].unwrap(),
                try_vec![None].unwrap(),
                256,
                64,
                io.clone(),
                crate::TempStore::Default,
            )
            .unwrap();

            let num_records = 1000 + rng.next_u64() % 2000;
            let num_records = num_records as i64;

            let num_values = 1 + rng.next_u64() % 4;
            let value_types = generate_value_types(&mut rng, num_values as usize);

            let mut initial_records = Vec::with_capacity(num_records as usize);
            for i in (0..num_records).rev() {
                let mut values = try_vec![Value::from_i64(i)].unwrap();
                values.append(&mut generate_values(&mut rng, &value_types));
                let record = ImmutableRecord::from_values(&values, values.len()).unwrap();

                io.block(|| sorter.insert(&record))
                    .expect("Failed to insert the record");
                initial_records.push(record);
            }

            io.block(|| sorter.sort())
                .expect("Failed to sort the records");

            assert!(!sorter.is_empty());
            assert!(!sorter.chunks.is_empty());

            for i in 0..num_records {
                assert!(sorter.has_more());
                let record = sorter.record().unwrap();
                assert_eq!(record.get_values().unwrap()[0], ValueRef::from_i64(i));
                // Check that the record remained unchanged after sorting.
                assert_eq!(record, &initial_records[(num_records - i - 1) as usize]);

                io.block(|| sorter.next())
                    .expect("Failed to get the next record");
            }
            assert!(!sorter.has_more());
        }
    }

    #[test]
    fn spilled_sort_splits_memory_budget_between_chunk_read_buffers() {
        let io = Arc::new(PlatformIO::new().unwrap());
        let max_buffer_size = 2000 * 1024;
        let min_read_buffer_size = 4096;
        let mut sorter = Sorter::new(
            &[SortOrder::Asc],
            try_vec![CollationSeq::Binary].unwrap(),
            try_vec![None].unwrap(),
            try_vec![None].unwrap(),
            max_buffer_size,
            min_read_buffer_size,
            io.clone(),
            crate::TempStore::Default,
        )
        .unwrap();

        let n = 4000;
        for i in (0..n).rev() {
            let values =
                try_vec![Value::from_i64(i), Value::Blob(try_vec![0; 1000].unwrap())].unwrap();
            let record = ImmutableRecord::from_values(&values, values.len()).unwrap();
            io.block(|| sorter.insert(&record))
                .expect("Failed to insert the record");
        }

        assert!(sorter.arena.allocated_bytes() > 0);
        assert!(sorter.records.capacity() > 0);

        io.block(|| sorter.sort())
            .expect("Failed to sort the records");
        assert_eq!(
            sorter.arena.allocated_bytes(),
            0,
            "input arena must be released before merging"
        );
        assert_eq!(
            sorter.records.capacity(),
            0,
            "input record capacity must be released before merging"
        );
        assert!(sorter.chunks.len() > 1);
        let read_buffer_size = max_buffer_size / sorter.chunks.len();
        assert!(read_buffer_size > min_read_buffer_size);
        for chunk in sorter.chunks.iter() {
            assert!(chunk.buffer.read().len() >= read_buffer_size.min(chunk.chunk_size));
        }

        for i in 0..n {
            assert!(sorter.has_more());
            let record = sorter.record().unwrap();
            assert_eq!(record.get_values().unwrap()[0], ValueRef::from_i64(i));
            io.block(|| sorter.next())
                .expect("Failed to get the next record");
        }
        assert!(!sorter.has_more());
    }

    #[cfg(all(
        feature = "io_memory_yield",
        feature = "fs",
        not(target_family = "wasm")
    ))]
    #[test]
    fn spilled_sort_resumes_heap_initialization_after_prefetch() {
        for num_records in [3, 6, 7] {
            let io = Arc::new(crate::io::MemoryYieldIO::new());
            let mut sorter = Sorter::new(
                &[SortOrder::Asc],
                try_vec![CollationSeq::Binary].unwrap(),
                try_vec![None].unwrap(),
                try_vec![None].unwrap(),
                2000 * 1024,
                4096,
                io.clone(),
                crate::TempStore::File,
            )
            .unwrap();
            let payload = try_vec![0; 800_000].unwrap();
            let mut initial_records = Vec::new();
            for i in (0..num_records).rev() {
                let values =
                    try_vec![Value::from_i64(i), Value::from_slice(&payload).unwrap()].unwrap();
                let record = ImmutableRecord::from_values(&values, values.len()).unwrap();
                io.block(|| sorter.insert(&record)).unwrap();
                initial_records.push(record);
            }

            let mut prefetch_yields = 0;
            loop {
                match sorter.sort().unwrap() {
                    IOResult::Done(()) => break,
                    IOResult::IO(completion) => {
                        if sorter.chunks.iter().any(|chunk| {
                            matches!(chunk.next_state, NextState::Finish)
                                && *chunk.io_state.read() == SortedChunkIOState::WaitingForRead
                        }) {
                            prefetch_yields += 1;
                        }
                        completion.wait(io.as_ref()).unwrap();
                    }
                }
            }
            assert!(prefetch_yields > 0);
            assert!(sorter.chunks.len() > 1);

            for expected in initial_records.iter().rev() {
                assert_eq!(sorter.record(), Some(expected));
                io.block(|| sorter.next()).unwrap();
            }
            assert!(!sorter.has_more());
        }
    }

    #[cfg(all(
        feature = "io_memory_yield",
        feature = "fs",
        not(target_family = "wasm")
    ))]
    #[test]
    fn spilled_sort_buffer_growth_failure_does_not_leave_pending_reads() {
        let io = Arc::new(crate::io::MemoryYieldIO::new());
        let mut sorter = Sorter::new(
            &[SortOrder::Asc],
            try_vec![CollationSeq::Binary].unwrap(),
            try_vec![None].unwrap(),
            try_vec![None].unwrap(),
            2048,
            64,
            io.clone(),
            crate::TempStore::File,
        )
        .unwrap();
        let values = try_vec![Value::Blob(try_vec![0; 1000].unwrap())].unwrap();
        let record = ImmutableRecord::from_values(&values, values.len()).unwrap();
        for _ in 0..3 {
            io.block(|| sorter.insert(&record)).unwrap();
        }
        let completion = sorter.flush().unwrap().unwrap();
        io.drain_completions(&[completion]).unwrap();
        assert_eq!(sorter.chunks.len(), 2);
        assert!(sorter.chunks[0].buffer.read().len() < sorter.chunks[0].chunk_size);

        sorter.max_buffer_size = usize::MAX;
        sorter.chunks[1].chunk_size = usize::MAX;
        let buffers: Vec<_> = sorter
            .chunks
            .iter()
            .map(|chunk| Arc::downgrade(&chunk.buffer))
            .try_collect()
            .unwrap();
        let error = sorter.init_chunk_heap().unwrap_err();
        assert!(matches!(*error, LimboError::OutOfMemory));
        assert_eq!(
            sorter.chunks[0].buffer.read().len(),
            sorter.chunks[0].chunk_size
        );
        drop(sorter);
        for buffer in buffers {
            assert!(
                buffer.upgrade().is_none(),
                "pending read retained its buffer"
            );
        }
    }

    fn generate_value_types<R: RngCore>(rng: &mut R, num_values: usize) -> Vec<ValueType> {
        let mut value_types = <Vec<ValueType> as TursoVecExt<ValueType>>::with_capacity(num_values);

        for _ in 0..num_values {
            let value_type: ValueType = match rng.next_u64() % 4 {
                0 => ValueType::Integer,
                1 => ValueType::Float,
                2 => ValueType::Blob,
                3 => ValueType::Null,
                _ => unreachable!(),
            };
            value_types.push(value_type);
        }

        value_types
    }

    fn generate_values<R: RngCore>(rng: &mut R, value_types: &[ValueType]) -> Vec<Value> {
        let mut values = <Vec<Value> as TursoVecExt<Value>>::with_capacity(value_types.len());
        for value_type in value_types {
            let value = match value_type {
                ValueType::Integer => Value::from_i64(rng.next_u64() as i64),
                ValueType::Float => {
                    let numerator = rng.next_u64() as f64;
                    let denominator = rng.next_u64() as f64;
                    Value::from_f64(numerator / denominator)
                }
                ValueType::Blob => {
                    let mut blob = <Vec<u8> as TursoVecExt<u8>>::with_capacity(
                        (rng.next_u64() % 2047 + 1) as usize,
                    );
                    rng.fill_bytes(&mut blob);
                    Value::Blob(blob)
                }
                ValueType::Null => Value::Null,
                _ => unreachable!(),
            };
            values.push(value);
        }
        values
    }

    fn assert_secondary_key_sort(
        second_order: SortOrder,
        second_nulls: Option<amphora_engine_parser::ast::NullsOrder>,
        seconds: &[Value],
        expected: &[ValueRef],
    ) {
        let io = Arc::new(PlatformIO::new().unwrap());
        let mut sorter = Sorter::new(
            &[SortOrder::Asc, second_order],
            try_vec![CollationSeq::Binary, CollationSeq::Binary].unwrap(),
            try_vec![None, second_nulls].unwrap(),
            try_vec![None, None].unwrap(),
            1 << 20,
            64,
            io.clone(),
            crate::TempStore::Default,
        )
        .unwrap();

        for second in seconds {
            let values = try_vec![Value::from_i64(1), second.clone()].unwrap();
            let record = ImmutableRecord::from_values(&values, values.len()).unwrap();
            io.block(|| sorter.insert(&record))
                .expect("Failed to insert the record");
        }

        io.block(|| sorter.sort())
            .expect("Failed to sort the records");
        assert!(sorter.chunks.is_empty());

        let mut idx = 0;
        while sorter.has_more() {
            {
                let record = sorter.record().unwrap();
                let vals = record.get_values().unwrap();
                assert_eq!(vals[0], ValueRef::from_i64(1));
                assert_eq!(vals[1], expected[idx]);
            }
            idx += 1;
            io.block(|| sorter.next())
                .expect("Failed to get the next record");
        }
        assert_eq!(idx, expected.len());
    }

    #[test]
    fn spilled_sort_orders_secondary_key_across_chunks() {
        let io = Arc::new(PlatformIO::new().unwrap());
        let mut sorter = Sorter::new(
            &[SortOrder::Asc, SortOrder::Asc],
            try_vec![CollationSeq::Binary, CollationSeq::Binary].unwrap(),
            try_vec![None, None].unwrap(),
            try_vec![None, None].unwrap(),
            // Tiny buffer so the sorter spills to multiple chunk files.
            256,
            64,
            io.clone(),
            crate::TempStore::Default,
        )
        .unwrap();

        let n = 200;
        // Equal first key, ascending second key on insert.
        for x in 0..n {
            let values = try_vec![Value::from_i64(1), Value::from_i64(x)].unwrap();
            let record = ImmutableRecord::from_values(&values, values.len()).unwrap();
            io.block(|| sorter.insert(&record))
                .expect("Failed to insert the record");
        }

        io.block(|| sorter.sort())
            .expect("Failed to sort the records");
        assert!(
            !sorter.chunks.is_empty(),
            "test requires the sorter to have spilled to chunks"
        );

        let mut idx = 0;
        while sorter.has_more() {
            {
                let record = sorter.record().unwrap();
                let vals = record.get_values().unwrap();
                assert_eq!(vals[0], ValueRef::from_i64(1));
                assert_eq!(
                    vals[1],
                    ValueRef::from_i64(idx),
                    "secondary key out of order at position {idx}"
                );
            }
            idx += 1;
            io.block(|| sorter.next())
                .expect("Failed to get the next record");
        }
        assert_eq!(idx, n);
    }

    #[test]
    fn spilled_sort_places_nulls_last_across_chunks() {
        let io = Arc::new(PlatformIO::new().unwrap());
        let mut sorter = Sorter::new(
            &[SortOrder::Asc, SortOrder::Asc],
            try_vec![CollationSeq::Binary, CollationSeq::Binary].unwrap(),
            try_vec![None, Some(amphora_engine_parser::ast::NullsOrder::Last)].unwrap(),
            try_vec![None, None].unwrap(),
            256,
            64,
            io.clone(),
            crate::TempStore::Default,
        )
        .unwrap();

        let n = 200;
        for x in 0..n {
            let second = if x % 2 == 0 {
                Value::Null
            } else {
                Value::from_i64(x)
            };
            let values = try_vec![Value::from_i64(1), second].unwrap();
            let record = ImmutableRecord::from_values(&values, values.len()).unwrap();
            io.block(|| sorter.insert(&record)).unwrap();
        }

        io.block(|| sorter.sort()).unwrap();
        assert!(
            !sorter.chunks.is_empty(),
            "test requires the sorter to have spilled to chunks"
        );

        let mut idx = 0;
        let mut prev = None;
        while sorter.has_more() {
            {
                let record = sorter.record().unwrap();
                let vals = record.get_values().unwrap();
                assert_eq!(vals[0], ValueRef::from_i64(1));
                match vals[1] {
                    ValueRef::Null => {
                        assert!(idx >= n / 2, "NULL emitted before all non-NULL values");
                    }
                    v => {
                        assert!(idx < n / 2, "non-NULL value {v:?} emitted after NULL block");
                        let current = match v {
                            ValueRef::Numeric(crate::numeric::Numeric::Integer(i)) => i,
                            other => panic!("unexpected value {other:?}"),
                        };
                        if let Some(prev) = prev {
                            assert!(current > prev);
                        }
                        prev = Some(current);
                    }
                }
            }
            idx += 1;
            io.block(|| sorter.next()).unwrap();
        }
        assert_eq!(idx, n);
    }

    #[test]
    fn in_memory_sort_applies_desc_on_secondary_key() {
        let seconds = try_vec![
            Value::from_i64(10),
            Value::from_i64(40),
            Value::from_i64(20),
            Value::from_i64(30)
        ]
        .unwrap();
        assert_secondary_key_sort(
            SortOrder::Desc,
            None,
            &seconds,
            &[
                ValueRef::from_i64(40),
                ValueRef::from_i64(30),
                ValueRef::from_i64(20),
                ValueRef::from_i64(10),
            ],
        );
    }

    #[test]
    fn in_memory_sort_places_nulls_last_on_desc_secondary_key() {
        let seconds = try_vec![
            Value::Null,
            Value::from_i64(10),
            Value::Null,
            Value::from_i64(20)
        ]
        .unwrap();
        assert_secondary_key_sort(
            SortOrder::Desc,
            Some(amphora_engine_parser::ast::NullsOrder::Last),
            &seconds,
            &[
                ValueRef::from_i64(20),
                ValueRef::from_i64(10),
                ValueRef::Null,
                ValueRef::Null,
            ],
        );
    }

    /// P2 regression: resetting a top-K sorter for statement-cache reuse must not retain prior
    /// result payloads or blobs while the statement sits idle in the cache.
    #[test]
    fn p2_top_k_sorter_reset_drops_prior_result_payloads() {
        let io: Arc<dyn IO> = Arc::new(crate::MemoryIO::new());
        let mut sorter = Sorter::new(
            &[SortOrder::Asc],
            std::vec![CollationSeq::Binary],
            std::vec![None],
            std::vec![None],
            64 * 1024,
            4 * 1024,
            io.clone(),
            crate::TempStore::Default,
        )
        .unwrap();

        sorter.limit_to(Some(5));
        for i in (0..40i64).rev() {
            let blob = std::vec![u8::try_from(i).unwrap(); 4096];
            let rec =
                ImmutableRecord::from_values(&[Value::from_i64(i), Value::Blob(blob)], 2).unwrap();
            io.block(|| sorter.insert(&rec)).unwrap();
        }
        io.block(|| sorter.sort()).unwrap();
        let mut first_run = Vec::new();
        while sorter.has_more() {
            let vals = sorter.record().unwrap().get_values().unwrap();
            first_run.push(vals[0].as_int().unwrap());
            io.block(|| sorter.next()).unwrap();
        }
        assert_eq!(first_run, std::vec![0, 1, 2, 3, 4]);

        sorter.clear_for_reuse();
        assert!(
            sorter.kept.is_none(),
            "top-K sorter must release kept records and blobs on reset"
        );
        assert!(
            sorter.current.is_none(),
            "top-K sorter must release current record on reset"
        );

        // Reusing the same sorter on a second top-K run still produces exact results.
        sorter.limit_to(Some(3));
        for i in [9i64, 2, 7, 1, 5] {
            let rec = ImmutableRecord::from_values(&[Value::from_i64(i)], 1).unwrap();
            io.block(|| sorter.insert(&rec)).unwrap();
        }
        io.block(|| sorter.sort()).unwrap();
        let mut second_run = Vec::new();
        while sorter.has_more() {
            let vals = sorter.record().unwrap().get_values().unwrap();
            second_run.push(vals[0].as_int().unwrap());
            io.block(|| sorter.next()).unwrap();
        }
        assert_eq!(second_run, std::vec![1, 2, 5]);
    }

    /// Review blocker 4 regression: a sorter whose bump arena grew past `KEPT_EPHEMERAL_BYTES`
    /// must refuse statement-reset reuse so finished statements never retain megabytes of uncharged
    /// sort memory while idle.
    #[test]
    fn review_large_sorter_refuses_reuse_to_bound_idle_statement_memory() {
        let io: Arc<dyn IO> = Arc::new(crate::MemoryIO::new());
        let mut sorter = Sorter::new(
            &[SortOrder::Asc],
            std::vec![CollationSeq::Binary],
            std::vec![None],
            std::vec![None],
            4 * 1024 * 1024,
            4 * 1024,
            io.clone(),
            crate::TempStore::Default,
        )
        .unwrap();

        // Small sort stays within KEPT_EPHEMERAL_BYTES and remains reusable.
        for i in 0..8i64 {
            let rec = ImmutableRecord::from_values(&[Value::from_i64(i)], 1).unwrap();
            io.block(|| sorter.insert(&rec)).unwrap();
        }
        assert!(sorter.can_reuse());

        // Large sort exceeds KEPT_EPHEMERAL_BYTES and must NOT be retained in spare_sorters.
        let wide = std::vec![b'a'; 2048];
        for i in 0..200i64 {
            let rec =
                ImmutableRecord::from_values(&[Value::from_i64(i), Value::Blob(wide.clone())], 2)
                    .unwrap();
            io.block(|| sorter.insert(&rec)).unwrap();
        }
        assert!(sorter.allocated_bytes() > crate::vdbe::KEPT_EPHEMERAL_BYTES);
        assert!(
            !sorter.can_reuse(),
            "large sorter ({} bytes) must be dropped on reset, not kept idle",
            sorter.allocated_bytes()
        );
    }

    /// Review P1 regression: evicting wide rows with tiny rows in an active top-K sorter must
    /// release the oversized evicted record buffers instead of retaining peak capacities across
    /// every heap slot for the lifetime of the query.
    #[test]
    fn review_top_k_sorter_releases_oversized_evicted_buffers_during_sort() {
        let io: Arc<dyn IO> = Arc::new(crate::MemoryIO::new());
        let mut sorter = Sorter::new(
            &[SortOrder::Asc],
            std::vec![CollationSeq::Binary],
            std::vec![None],
            std::vec![None],
            2 * 1024 * 1024,
            4 * 1024,
            io.clone(),
            crate::TempStore::Default,
        )
        .unwrap();

        sorter.limit_to(Some(16));
        let wide = std::vec![b'w'; 64 * 1024];
        for i in 100..116i64 {
            let rec =
                ImmutableRecord::from_values(&[Value::from_i64(i), Value::Blob(wide.clone())], 2)
                    .unwrap();
            io.block(|| sorter.insert(&rec)).unwrap();
        }
        assert!(sorter.kept.is_some());
        // Replace all 16 wide rows with tiny rows that sort earlier.
        for i in 0..16i64 {
            let rec = ImmutableRecord::from_values(
                &[Value::from_i64(i), Value::Blob(std::vec![1u8; 8])],
                2,
            )
            .unwrap();
            io.block(|| sorter.insert(&rec)).unwrap();
        }
        let retained_bytes: usize = sorter
            .kept
            .as_ref()
            .unwrap()
            .heap
            .iter()
            .map(|k| k.record.record.as_blob().capacity())
            .sum();
        assert!(
            retained_bytes < 4096,
            "evicted 64 KiB buffers must be released during active sort; retained {retained_bytes} bytes"
        );
        io.block(|| sorter.sort()).unwrap();
        let mut out = Vec::new();
        while sorter.has_more() {
            let vals = sorter.record().unwrap().get_values().unwrap();
            out.push(vals[0].as_int().unwrap());
            io.block(|| sorter.next()).unwrap();
        }
        assert_eq!(out, (0..16i64).collect::<Vec<_>>());
    }

    /// Review P2 regression: active top-K winners that exceed the sorter's byte budget must spill
    /// payloads rather than retaining unbounded memory in `Kept`, using key-only `SpilledTopK`
    /// when keys fit and chunk-based external merge when the sort keys themselves exceed the
    /// budget.
    #[test]
    fn review_top_k_sorter_spills_when_winners_exceed_byte_budget() {
        let io: Arc<dyn IO> = Arc::new(crate::MemoryIO::new());
        let budget = 64 * 1024;
        // Case 1: small integer sort key, wide 32 KiB payload -> key-only SpilledTopK.
        let mut sorter = Sorter::new(
            &[SortOrder::Asc],
            std::vec![CollationSeq::Binary],
            std::vec![None],
            std::vec![None],
            budget,
            4 * 1024,
            io.clone(),
            crate::TempStore::Memory,
        )
        .unwrap();

        sorter.limit_to(Some(16));
        let wide = std::vec![b'w'; 32 * 1024];
        for i in (0..24i64).rev() {
            let rec =
                ImmutableRecord::from_values(&[Value::from_i64(i), Value::Blob(wide.clone())], 2)
                    .unwrap();
            io.block(|| sorter.insert(&rec)).unwrap();
            assert!(
                sorter.allocated_bytes() <= budget,
                "allocated sorter bytes {} exceeded budget {budget}",
                sorter.allocated_bytes()
            );
        }
        assert!(
            sorter.kept.is_none() && sorter.spilled_top_k.is_some() && sorter.chunks.is_empty(),
            "small-key top-K sorter must spill wide payloads via SpilledTopK"
        );
        io.block(|| sorter.sort()).unwrap();
        let mut out = Vec::new();
        while sorter.has_more() {
            let vals = sorter.record().unwrap().get_values().unwrap();
            out.push(vals[0].as_int().unwrap());
            io.block(|| sorter.next()).unwrap();
        }
        assert_eq!(out, (0..16i64).collect::<Vec<_>>());

        // Case 2: giant sort key itself exceeding the byte budget -> stays in SpilledTopK with
        // out-of-line keys (`inline_keys = None`, `chunks.is_empty()`), keeping RAM `O(1)`.
        let mut big_key = Sorter::new(
            &[SortOrder::Asc],
            std::vec![CollationSeq::Binary],
            std::vec![None],
            std::vec![None],
            budget,
            4 * 1024,
            io.clone(),
            crate::TempStore::Memory,
        )
        .unwrap();
        big_key.limit_to(Some(16));
        for i in (0..6u8).rev() {
            let mut key_blob = std::vec![i; 32 * 1024];
            key_blob[0] = i;
            let rec = ImmutableRecord::from_values(&[Value::Blob(key_blob)], 1).unwrap();
            io.block(|| big_key.insert(&rec)).unwrap();
            assert!(
                big_key.allocated_bytes() <= budget,
                "allocated bytes {} exceeded budget {budget}",
                big_key.allocated_bytes()
            );
        }
        assert!(
            big_key.kept.is_none() && big_key.spilled_top_k.is_some() && big_key.chunks.is_empty(),
            "giant-key top-K sorter must stay in SpilledTopK with out-of-line keys"
        );
        io.block(|| big_key.sort()).unwrap();
        let mut key_out = Vec::new();
        while big_key.has_more() {
            let vals = big_key.record().unwrap().get_values().unwrap();
            key_out.push(vals[0].as_blob()[0]);
            io.block(|| big_key.next()).unwrap();
        }
        assert_eq!(key_out, std::vec![0, 1, 2, 3, 4, 5]);
    }

    /// Spec P1 regression: external merge across spilled chunks must break equal-key ties in
    /// arrival order (`chunk 0` before `chunk 1`), matching the in-memory stable sort contract.
    #[test]
    fn p1_spilled_merge_preserves_equal_key_arrival_order() {
        let io = Arc::new(PlatformIO::new().unwrap());
        let mut sorter = Sorter::new(
            &[SortOrder::Asc],
            std::vec![CollationSeq::Binary],
            std::vec![Some(amphora_engine_parser::ast::NullsOrder::Last)],
            std::vec![None],
            64,
            32,
            io.clone(),
            crate::TempStore::File,
        )
        .unwrap();
        // Three rows with identical sort key `k = 1`, each larger than `max_buffer_size = 64` so
        // every row flushes into its own spill run (chunks 0, 1, 2).
        for id in [1i64, 2, 3] {
            let pad = std::vec![u8::try_from(id).unwrap(); 100];
            let rec = ImmutableRecord::from_values(
                &[Value::from_i64(1), Value::from_i64(id), Value::Blob(pad)],
                3,
            )
            .unwrap();
            io.block(|| sorter.insert(&rec)).unwrap();
        }
        io.block(|| sorter.sort()).unwrap();
        assert!(
            sorter.chunks.len() >= 2,
            "test requires multiple spilled runs"
        );
        let mut ids = Vec::new();
        while sorter.has_more() {
            let vals = sorter.record().unwrap().get_values().unwrap();
            ids.push(vals[1].as_int().unwrap());
            io.block(|| sorter.next()).unwrap();
        }
        assert_eq!(ids, std::vec![1, 2, 3]);
    }

    /// Standards P1 regression: a file-backed `LIMIT 2` sort with wide 520 KiB payloads must keep
    /// `O(1)` live memory regardless of input count (4 rows vs 12 rows), never retaining a read
    /// buffer per input run.
    #[test]
    fn p1_file_backed_top_k_sort_keeps_o1_memory_independent_of_input_rows() {
        let io = Arc::new(PlatformIO::new().unwrap());
        let budget = 64 * 1024;
        for row_count in [4i64, 12] {
            let mut sorter = Sorter::new(
                &[SortOrder::Asc],
                std::vec![CollationSeq::Binary],
                std::vec![Some(amphora_engine_parser::ast::NullsOrder::Last)],
                std::vec![None],
                budget,
                4096,
                io.clone(),
                crate::TempStore::File,
            )
            .unwrap();
            sorter.limit_to(Some(2));
            for id in 1..=row_count {
                let key = row_count - id + 1;
                let payload = std::vec![u8::try_from(id).unwrap(); 520_000];
                let rec = ImmutableRecord::from_values(
                    &[
                        Value::from_i64(key),
                        Value::from_i64(id),
                        Value::Blob(payload),
                    ],
                    3,
                )
                .unwrap();
                io.block(|| sorter.insert(&rec)).unwrap();
                assert!(
                    sorter.allocated_bytes() < budget,
                    "sorter retained {} bytes during insert of row {id}/{row_count}",
                    sorter.allocated_bytes()
                );
            }
            assert!(sorter.spilled_top_k.is_some());
            assert!(sorter.chunks.is_empty());
            io.block(|| sorter.sort()).unwrap();
            assert!(
                sorter.allocated_bytes() < budget,
                "sorter retained {} bytes at first output for {row_count} rows",
                sorter.allocated_bytes()
            );
            let mut out = Vec::new();
            while sorter.has_more() {
                let vals = sorter.record().unwrap().get_values().unwrap();
                out.push((vals[0].as_int().unwrap(), vals[1].as_int().unwrap()));
                io.block(|| sorter.next()).unwrap();
            }
            assert_eq!(out, std::vec![(1, row_count), (2, row_count - 1)]);
        }
    }

    /// Standards P2 regression: replacing a smaller kept row with a larger row that still fits the
    /// byte budget must allocate exact capacity instead of letting `Vec::reserve` double past
    /// `max_buffer_size`.
    #[test]
    fn p2_top_k_replacement_does_not_double_past_byte_budget() {
        let io: Arc<dyn IO> = Arc::new(crate::MemoryIO::new());
        let budget = 2500;
        let mut sorter = Sorter::new(
            &[SortOrder::Asc],
            std::vec![CollationSeq::Binary],
            std::vec![None],
            std::vec![None],
            budget,
            64,
            io.clone(),
            crate::TempStore::Memory,
        )
        .unwrap();
        sorter.limit_to(Some(1));
        let first = ImmutableRecord::from_values(
            &[Value::from_i64(2), Value::Blob(std::vec![b'a'; 1200])],
            2,
        )
        .unwrap();
        io.block(|| sorter.insert(&first)).unwrap();

        let second = ImmutableRecord::from_values(
            &[Value::from_i64(1), Value::Blob(std::vec![b'b'; 1600])],
            2,
        )
        .unwrap();
        io.block(|| sorter.insert(&second)).unwrap();

        assert!(
            sorter.allocated_bytes() <= budget,
            "replacement doubled past {budget}-byte budget to {} bytes",
            sorter.allocated_bytes()
        );
        let kept = sorter
            .kept
            .as_ref()
            .expect("1600-byte winner fits in 2500-byte budget without doubling");
        let cap = kept.heap.peek().unwrap().record.record.as_blob().capacity();
        assert_eq!(cap, second.get_payload().len());
        assert_eq!(kept.bytes, cap);
    }

    /// Spec P2 regression: if spill conversion fails partway through, previously accepted winners
    /// remain intact in `Kept`.
    #[test]
    fn p2_failed_spill_conversion_preserves_previously_accepted_winners() {
        let io: Arc<dyn IO> = Arc::new(crate::MemoryIO::new());
        let budget = 1024;
        let mut sorter = Sorter::new(
            &[SortOrder::Asc],
            std::vec![CollationSeq::Binary],
            std::vec![None],
            std::vec![None],
            budget,
            64,
            io.clone(),
            crate::TempStore::Memory,
        )
        .unwrap();
        sorter.limit_to(Some(2));
        for (k, id) in [(10i64, 1i64), (20, 2)] {
            let rec = ImmutableRecord::from_values(&[Value::from_i64(k), Value::from_i64(id)], 2)
                .unwrap();
            io.block(|| sorter.insert(&rec)).unwrap();
        }
        // Trigger `demote_kept_to_spilled_top_k` with a malformed 0-column incoming record
        // whose payload exceeds the budget (forces an error during conversion).
        let mut bad_payload = std::vec![1u8]; // header len 1, 0 columns
        bad_payload.resize(2048, 0);
        let mut bad = ImmutableRecord::new(bad_payload.len()).unwrap();
        bad.start_serialization(&bad_payload).unwrap();
        let err = sorter.demote_kept_to_spilled_top_k(&bad);
        assert!(err.is_err());

        // Previously accepted winners must still be present and sortable.
        assert_eq!(sorter.kept.as_ref().unwrap().heap.len(), 2);
        io.block(|| sorter.sort()).unwrap();
        let mut out = Vec::new();
        while sorter.has_more() {
            let vals = sorter.record().unwrap().get_values().unwrap();
            out.push((vals[0].as_int().unwrap(), vals[1].as_int().unwrap()));
            io.block(|| sorter.next()).unwrap();
        }
        assert_eq!(out, std::vec![(10, 1), (20, 2)]);
    }

    /// Standards P1 regression: a file-backed `LIMIT 2` sort with 520 KiB sort keys (including
    /// shared prefixes that force full key comparisons) must keep `O(1)` RAM and `O(K)` temp-file
    /// growth regardless of whether 4 or 12 rows arrive.
    #[test]
    fn p1_file_backed_top_k_wide_keys_keep_o1_memory_and_bounded_temp_file() {
        let io = Arc::new(PlatformIO::new().unwrap());
        let budget = 64 * 1024;
        let mut watermarks = Vec::new();
        for row_count in [4u8, 12] {
            let mut sorter = Sorter::new(
                &[SortOrder::Asc],
                std::vec![CollationSeq::Binary],
                std::vec![Some(amphora_engine_parser::ast::NullsOrder::Last)],
                std::vec![None],
                budget,
                4096,
                io.clone(),
                crate::TempStore::File,
            )
            .unwrap();
            sorter.limit_to(Some(2));
            let mut rec_len = 0usize;
            for id in 1..=row_count {
                // Identical 8-byte prefix forces full-key tie-break reads on disk; the last byte
                // descends with `id` so every row beats the current heap worst.
                let mut key_blob = std::vec![b'p'; 520_000];
                key_blob[519_999] = row_count - id + 1;
                let rec = ImmutableRecord::from_values(
                    &[Value::Blob(key_blob), Value::from_i64(i64::from(id))],
                    2,
                )
                .unwrap();
                rec_len = rec.get_payload().len();
                io.block(|| sorter.insert(&rec)).unwrap();
                assert!(
                    sorter.allocated_bytes() < budget,
                    "wide-key sorter retained {} bytes on row {id}/{row_count}",
                    sorter.allocated_bytes()
                );
            }
            let spilled = sorter
                .spilled_top_k
                .as_ref()
                .expect("wide-key LIMIT 2 must use SpilledTopK");
            assert!(sorter.chunks.is_empty());
            let slot_cap = aligned_file_slot_cap(rec_len).unwrap() as u64;
            assert!(
                spilled.next_offset <= slot_cap * 3,
                "temp file watermark {} exceeded 3 * slot_cap ({}) for {row_count} rows",
                spilled.next_offset,
                slot_cap * 3
            );
            watermarks.push(spilled.next_offset);

            io.block(|| sorter.sort()).unwrap();
            assert!(sorter.allocated_bytes() < budget);
            let mut ids = Vec::new();
            while sorter.has_more() {
                let vals = sorter.record().unwrap().get_values().unwrap();
                ids.push(vals[1].as_int().unwrap());
                io.block(|| sorter.next()).unwrap();
            }
            assert_eq!(
                ids,
                std::vec![i64::from(row_count), i64::from(row_count - 1)]
            );
        }
        // Watermark for 12 rows must match watermark for 4 rows (`O(K)` slot reclamation).
        assert_eq!(watermarks[0], watermarks[1]);
    }

    /// Standards P1 regression: a file-backed `LIMIT 2` sort with narrow keys and successively
    /// better 520 KiB winners must reclaim evicted file slots instead of appending unboundedly.
    #[test]
    fn p1_file_backed_top_k_reclaims_evicted_temp_file_slots() {
        let io = Arc::new(PlatformIO::new().unwrap());
        let budget = 64 * 1024;
        let mut sorter = Sorter::new(
            &[SortOrder::Asc],
            std::vec![CollationSeq::Binary],
            std::vec![None],
            std::vec![None],
            budget,
            4096,
            io.clone(),
            crate::TempStore::File,
        )
        .unwrap();
        sorter.limit_to(Some(2));
        let mut rec_len = 0usize;
        for step in 0..12i64 {
            let key = 100 - step;
            let payload = std::vec![u8::try_from(step).unwrap(); 520_000];
            let rec = ImmutableRecord::from_values(
                &[
                    Value::from_i64(key),
                    Value::from_i64(step),
                    Value::Blob(payload),
                ],
                3,
            )
            .unwrap();
            rec_len = rec.get_payload().len();
            io.block(|| sorter.insert(&rec)).unwrap();
        }
        let spilled = sorter.spilled_top_k.as_ref().unwrap();
        let slot_cap = aligned_file_slot_cap(rec_len).unwrap() as u64;
        assert!(
            spilled.next_offset <= slot_cap * 3,
            "narrow-key spilled top-K grew temp file to {} bytes (expected <= {} bytes = 3 slots)",
            spilled.next_offset,
            slot_cap * 3
        );
        io.block(|| sorter.sort()).unwrap();
        let mut out = Vec::new();
        while sorter.has_more() {
            let vals = sorter.record().unwrap().get_values().unwrap();
            out.push((vals[0].as_int().unwrap(), vals[1].as_int().unwrap()));
            io.block(|| sorter.next()).unwrap();
        }
        assert_eq!(out, std::vec![(89, 11), (90, 10)]);
    }

    /// Spec P1 regression: `SpilledTopK` must apply `cmp_with_sort` (`DESC` and `NULLS LAST`)
    /// around custom comparators, matching in-memory SQL ordering.
    #[test]
    fn p1_spilled_top_k_respects_desc_nulls_last_custom_comparator() {
        let io: Arc<dyn IO> = Arc::new(crate::MemoryIO::new());
        // Custom comparator ordering blobs by their first byte (ascending base order).
        let custom: SortComparator = Arc::new(|a: &ValueRef, b: &ValueRef| match (a, b) {
            (ValueRef::Null, ValueRef::Null) => Ok(Ordering::Equal),
            (ValueRef::Null, _) => Ok(Ordering::Less),
            (_, ValueRef::Null) => Ok(Ordering::Greater),
            (ValueRef::Blob(x), ValueRef::Blob(y)) => Ok(x.first().cmp(&y.first())),
            _ => Ok(Ordering::Equal),
        });
        let mut sorter = Sorter::new(
            &[SortOrder::Desc],
            std::vec![CollationSeq::Binary],
            std::vec![Some(amphora_engine_parser::ast::NullsOrder::Last)],
            std::vec![Some(custom)],
            1024,
            256,
            io.clone(),
            crate::TempStore::Memory,
        )
        .unwrap();
        sorter.limit_to(Some(2));

        let pad = std::vec![b'x'; 2048];
        for (id, key) in [
            (1i64, Value::Blob(std::vec![10u8])),
            (2, Value::Blob(std::vec![20u8])),
            (3, Value::Blob(std::vec![30u8])),
            (4, Value::Null),
        ] {
            let rec = ImmutableRecord::from_values(
                &[key, Value::from_i64(id), Value::Blob(pad.clone())],
                3,
            )
            .unwrap();
            io.block(|| sorter.insert(&rec)).unwrap();
        }
        assert!(sorter.spilled_top_k.is_some());
        io.block(|| sorter.sort()).unwrap();
        let mut ids = Vec::new();
        while sorter.has_more() {
            let vals = sorter.record().unwrap().get_values().unwrap();
            ids.push(vals[1].as_int().unwrap());
            io.block(|| sorter.next()).unwrap();
        }
        assert_eq!(ids, std::vec![3, 2]);
    }

    /// Standards P2 regression: a short write during spill demotion must leave previously accepted
    /// `Kept` winners intact for retry, and a short read during output must leave the pending
    /// winner on `SpilledTopK::sorted` so retrying `next()` returns it without data loss.
    #[test]
    fn p2_failed_demote_short_write_and_failed_read_preserve_winners_for_retry() {
        use std::sync::atomic::{AtomicBool, Ordering as AOrd};

        struct FaultFile {
            inner: Arc<dyn File>,
            short_write: Arc<AtomicBool>,
            short_read: Arc<AtomicBool>,
        }
        impl File for FaultFile {
            fn lock_file(&self, exclusive: bool) -> Result<()> {
                self.inner.lock_file(exclusive)
            }
            fn unlock_file(&self) -> Result<()> {
                self.inner.unlock_file()
            }
            fn pread(&self, pos: u64, c: Completion) -> Result<Completion> {
                if self.short_read.swap(false, AOrd::SeqCst) {
                    c.complete(0);
                    return Ok(c);
                }
                self.inner.pread(pos, c)
            }
            fn pwrite(&self, pos: u64, buffer: Arc<Buffer>, c: Completion) -> Result<Completion> {
                if self.short_write.swap(false, AOrd::SeqCst) {
                    c.complete(0);
                    return Ok(c);
                }
                self.inner.pwrite(pos, buffer, c)
            }
            fn sync(
                &self,
                c: Completion,
                sync_type: crate::io::FileSyncType,
            ) -> Result<Completion> {
                self.inner.sync(c, sync_type)
            }
            fn size(&self) -> Result<u64> {
                self.inner.size()
            }
            fn truncate(&self, len: u64, c: Completion) -> Result<Completion> {
                self.inner.truncate(len, c)
            }
        }

        let io: Arc<dyn IO> = Arc::new(crate::MemoryIO::new());
        let short_write = Arc::new(AtomicBool::new(true));
        let short_read = Arc::new(AtomicBool::new(false));
        let mut raw_tf = TempFile::with_temp_store(&io, crate::TempStore::Memory).unwrap();
        let fault_file: Arc<dyn File> = Arc::new(FaultFile {
            inner: raw_tf.file.clone(),
            short_write,
            short_read: short_read.clone(),
        });
        raw_tf.file = fault_file;

        let mut sorter = Sorter::new(
            &[SortOrder::Asc],
            std::vec![CollationSeq::Binary],
            std::vec![None],
            std::vec![None],
            1024,
            64,
            io.clone(),
            crate::TempStore::Memory,
        )
        .unwrap();
        sorter.temp_file = Some(raw_tf);
        sorter.limit_to(Some(2));

        // Insert two small winners that stay in `Kept`.
        for (k, id) in [(10i64, 1i64), (20, 2)] {
            let rec = ImmutableRecord::from_values(&[Value::from_i64(k), Value::from_i64(id)], 2)
                .unwrap();
            io.block(|| sorter.insert(&rec)).unwrap();
        }
        assert_eq!(sorter.kept.as_ref().unwrap().heap.len(), 2);

        // Third winner triggers demote, which encounters an injected short write.
        let big = ImmutableRecord::from_values(
            &[
                Value::from_i64(5),
                Value::from_i64(3),
                Value::Blob(std::vec![b'z'; 1200]),
            ],
            3,
        )
        .unwrap();
        let err = io.block(|| sorter.insert(&big));
        assert!(err.is_err(), "injected short write must fail demote");
        assert!(
            sorter.kept.as_ref().is_some_and(|k| k.heap.len() == 2),
            "previously accepted winners must remain intact in Kept after short write"
        );
        assert!(sorter.spilled_top_k.is_none());
        assert!(sorter.pending_demote.is_none());

        // Retrying the same insert now succeeds and retains the top 2 winners [(5, 3), (10, 1)].
        io.block(|| sorter.insert(&big)).unwrap();
        assert!(sorter.spilled_top_k.is_some());
        assert!(sorter.kept.is_none());

        // Inject a short read on the first output record during `sort()` -> `next()`.
        short_read.store(true, AOrd::SeqCst);
        let read_err = io.block(|| sorter.sort());
        assert!(read_err.is_err(), "injected short read must error");
        assert_eq!(
            sorter.spilled_top_k.as_ref().unwrap().heap.len(),
            2,
            "failed read must not pop the winner before populating current"
        );

        // Retrying `sort()` serves BOTH winners intact without losing (5, 3).
        io.block(|| sorter.sort()).unwrap();
        let mut out = Vec::new();
        while sorter.has_more() {
            let vals = sorter.record().unwrap().get_values().unwrap();
            out.push((vals[0].as_int().unwrap(), vals[1].as_int().unwrap()));
            io.block(|| sorter.next()).unwrap();
        }
        assert_eq!(out, std::vec![(5, 3), (10, 1)]);
    }

    /// Standards P1-2 regression: resolving wide-key prefix ties inside `SpilledTopK` must yield
    /// caller-driven `IOResult::IO` completions rather than blocking on `IOCompletions::wait(io)`.
    #[test]
    fn p1_spilled_top_k_prefix_tie_yields_async_io_without_blocking() {
        use std::sync::Mutex;

        type QueuedOp = Box<dyn FnOnce() + Send>;
        struct DeferredFile {
            inner: Arc<dyn File>,
            queue: Arc<Mutex<VecDeque<QueuedOp>>>,
        }
        impl File for DeferredFile {
            fn lock_file(&self, exclusive: bool) -> Result<()> {
                self.inner.lock_file(exclusive)
            }
            fn unlock_file(&self) -> Result<()> {
                self.inner.unlock_file()
            }
            fn pread(&self, pos: u64, c: Completion) -> Result<Completion> {
                let inner = self.inner.clone();
                let c_run = c.clone();
                self.queue.lock().unwrap().push_back(Box::new(move || {
                    drop(inner.pread(pos, c_run).unwrap());
                }));
                Ok(c)
            }
            fn pwrite(&self, pos: u64, buffer: Arc<Buffer>, c: Completion) -> Result<Completion> {
                let inner = self.inner.clone();
                let c_run = c.clone();
                self.queue.lock().unwrap().push_back(Box::new(move || {
                    drop(inner.pwrite(pos, buffer, c_run).unwrap());
                }));
                Ok(c)
            }
            fn sync(
                &self,
                c: Completion,
                sync_type: crate::io::FileSyncType,
            ) -> Result<Completion> {
                self.inner.sync(c, sync_type)
            }
            fn size(&self) -> Result<u64> {
                self.inner.size()
            }
            fn truncate(&self, len: u64, c: Completion) -> Result<Completion> {
                self.inner.truncate(len, c)
            }
        }

        struct NoBlockingStepIO {
            inner: Arc<crate::MemoryIO>,
        }
        impl IO for NoBlockingStepIO {
            fn open_file(
                &self,
                path: &str,
                flags: crate::OpenFlags,
                direct: bool,
            ) -> Result<Arc<dyn File>> {
                self.inner.open_file(path, flags, direct)
            }
            fn remove_file(&self, path: &str) -> Result<()> {
                self.inner.remove_file(path)
            }
            fn step(&self) -> Result<()> {
                panic!(
                    "SpilledTopK must never call io.step() / IOCompletions::wait() synchronously"
                );
            }
        }
        impl crate::Clock for NoBlockingStepIO {
            fn current_time_monotonic(&self) -> crate::MonotonicInstant {
                self.inner.current_time_monotonic()
            }
            fn current_time_wall_clock(&self) -> crate::WallClockInstant {
                self.inner.current_time_wall_clock()
            }
        }

        let mem_io: Arc<dyn IO> = Arc::new(crate::MemoryIO::new());
        let mut raw_tf = TempFile::with_temp_store(&mem_io, crate::TempStore::Memory).unwrap();
        let queue: Arc<Mutex<VecDeque<QueuedOp>>> = Arc::new(Mutex::new(VecDeque::new()));
        raw_tf.file = Arc::new(DeferredFile {
            inner: raw_tf.file.clone(),
            queue: queue.clone(),
        });

        let no_step_io: Arc<dyn IO> = Arc::new(NoBlockingStepIO {
            inner: Arc::new(crate::MemoryIO::new()),
        });
        let mut sorter = Sorter::new(
            &[SortOrder::Asc],
            std::vec![CollationSeq::Binary],
            std::vec![None],
            std::vec![None],
            4096,
            512,
            no_step_io,
            crate::TempStore::Memory,
        )
        .unwrap();
        sorter.temp_file = Some(raw_tf);
        sorter.limit_to(Some(2));

        let mut yield_count = 0usize;
        fn drive(
            queue: &Mutex<VecDeque<QueuedOp>>,
            yield_count: &mut usize,
            mut f: impl FnMut() -> IOResultOr<()>,
        ) {
            loop {
                match f().unwrap() {
                    IOResult::Done(()) => break,
                    IOResult::IO(_) => {
                        *yield_count += 1;
                        let op = queue
                            .lock()
                            .unwrap()
                            .pop_front()
                            .expect("deferred file op must be queued when yielding IO");
                        op();
                    }
                }
            }
        }

        // Wide keys with identical 8-byte prefixes force out-of-line prefix-tie reads during both
        // insert and finish_sort_async.
        for (id, suffix) in [(1i64, b'd'), (2, b'b'), (3, b'c'), (4, b'a')] {
            let mut key = std::vec![b'P'; 8192];
            *key.last_mut().unwrap() = suffix;
            let rec =
                ImmutableRecord::from_values(&[Value::Blob(key), Value::from_i64(id)], 2).unwrap();
            drive(&queue, &mut yield_count, || sorter.insert(&rec));
        }
        assert!(sorter.spilled_top_k.is_some());
        drive(&queue, &mut yield_count, || sorter.sort());
        let mut out_ids = Vec::new();
        while sorter.has_more() {
            let vals = sorter.record().unwrap().get_values().unwrap();
            out_ids.push(vals[1].as_int().unwrap());
            drive(&queue, &mut yield_count, || sorter.next());
        }
        assert_eq!(out_ids, std::vec![4, 2]);
        assert!(
            yield_count >= 4,
            "expected caller-driven async I/O yields on writes and prefix-tie reads; got {yield_count}"
        );
    }

    /// Standards P1-3 regression: a large-LIMIT sort (`LIMIT 8192` under a `102_400`-byte budget)
    /// must stay strictly within its configured memory budget during insert and sort instead of
    /// allocating an oversized top-K metadata heap and duplicate merge-sort index arrays.
    #[test]
    fn p1_large_limit_sort_stays_within_configured_memory_budget() {
        let io: Arc<dyn IO> = Arc::new(crate::MemoryIO::new());
        let budget = 102_400usize;
        let limit = 8_192usize;
        let mut sorter = Sorter::new(
            &[SortOrder::Asc],
            std::vec![CollationSeq::Binary],
            std::vec![None],
            std::vec![None],
            budget,
            64 * 1024,
            io.clone(),
            crate::TempStore::Memory,
        )
        .unwrap();
        sorter.limit_to(Some(limit));

        let mut peak_allocated = 0usize;
        for i in (0..12_000i64).rev() {
            let rec = ImmutableRecord::from_values(
                &[Value::from_i64(i), Value::build_text(format!("row-{i:06}"))],
                2,
            )
            .unwrap();
            io.block(|| sorter.insert(&rec)).unwrap();
            peak_allocated = peak_allocated.max(sorter.allocated_bytes());
        }
        assert!(
            sorter.spilled_top_k.is_none() && !sorter.chunks.is_empty(),
            "LIMIT 8192 under 100 KiB budget must demote to budget-bounded chunk merge"
        );
        io.block(|| sorter.sort()).unwrap();
        peak_allocated = peak_allocated.max(sorter.allocated_bytes());
        assert!(
            peak_allocated <= budget.saturating_mul(2),
            "peak allocated bytes {peak_allocated} exceeded bounded memory for budget {budget}"
        );

        let mut count = 0usize;
        let mut prev = -1i64;
        while sorter.has_more() {
            let vals = sorter.record().unwrap().get_values().unwrap();
            let k = vals[0].as_int().unwrap();
            assert_eq!(k, prev + 1);
            prev = k;
            count += 1;
            io.block(|| sorter.next()).unwrap();
        }
        assert_eq!(count, limit);
    }

    /// Standards P2-5 regression: if a fallible comparison fails while inserting into `SpilledTopK`,
    /// `self.heap` must remain completely untouched so retrying the failed insert never duplicates
    /// an entry (`[1, 2]`, never `[1, 2, 2]`).
    #[test]
    fn p2_retrying_failed_spilled_top_k_insert_never_duplicates_row() {
        use std::sync::atomic::{AtomicBool, Ordering as AOrd};

        let io: Arc<dyn IO> = Arc::new(crate::MemoryIO::new());
        let fail_next = Arc::new(AtomicBool::new(false));
        let fail_cb = fail_next.clone();
        let custom: SortComparator = Arc::new(move |a: &ValueRef, b: &ValueRef| {
            if fail_cb.swap(false, AOrd::SeqCst) {
                return Err(LimboError::InternalError(
                    "injected comparator fault".into(),
                ));
            }
            Ok(a.partial_cmp(b).unwrap_or(Ordering::Equal))
        });

        let mut sorter = Sorter::new(
            &[SortOrder::Asc],
            std::vec![CollationSeq::Binary],
            std::vec![None],
            std::vec![Some(custom)],
            512,
            128,
            io.clone(),
            crate::TempStore::Memory,
        )
        .unwrap();
        sorter.limit_to(Some(4));

        let pad = std::vec![b'x'; 1024];
        let r1 = ImmutableRecord::from_values(&[Value::from_i64(1), Value::Blob(pad.clone())], 2)
            .unwrap();
        let r2 = ImmutableRecord::from_values(&[Value::from_i64(2), Value::Blob(pad)], 2).unwrap();

        // Row 1 exceeds budget and enters `SpilledTopK` with 1 entry.
        io.block(|| sorter.insert(&r1)).unwrap();
        assert!(sorter.spilled_top_k.is_some());
        assert_eq!(sorter.spilled_top_k.as_ref().unwrap().heap.len(), 1);

        // Fail the comparison while inserting Row 2, then retry Row 2 cleanly.
        fail_next.store(true, AOrd::SeqCst);
        let err = io.block(|| sorter.insert(&r2));
        assert!(err.is_err(), "injected comparator fault must fail insert");
        assert_eq!(
            sorter.spilled_top_k.as_ref().unwrap().heap.len(),
            1,
            "failed SpilledTopK insert must not mutate heap before comparisons finish"
        );

        io.block(|| sorter.insert(&r2)).unwrap();
        assert_eq!(sorter.spilled_top_k.as_ref().unwrap().heap.len(), 2);

        io.block(|| sorter.sort()).unwrap();
        let mut out = Vec::new();
        while sorter.has_more() {
            let vals = sorter.record().unwrap().get_values().unwrap();
            out.push(vals[0].as_int().unwrap());
            io.block(|| sorter.next()).unwrap();
        }
        assert_eq!(out, std::vec![1, 2]);
    }

    /// Blocker 3 regression: when temp space is unavailable (`open_file` fails for temp files),
    /// `TempStore::Default` must fall back to in-memory temp storage so spilled top-K queries still
    /// succeed, while explicit `TempStore::File` continues to report the I/O error.
    #[test]
    fn blocker_temp_store_default_falls_back_to_memory_when_temp_space_unavailable() {
        struct NoTempSpaceIO;
        impl IO for NoTempSpaceIO {
            fn open_file(
                &self,
                _path: &str,
                _flags: crate::OpenFlags,
                _direct: bool,
            ) -> Result<Arc<dyn File>> {
                Err(
                    CompletionError::IOError(std::io::ErrorKind::StorageFull, "no temp space")
                        .into(),
                )
            }
            fn remove_file(&self, _path: &str) -> Result<()> {
                Ok(())
            }
            fn step(&self) -> Result<()> {
                Ok(())
            }
        }
        impl crate::Clock for NoTempSpaceIO {
            fn current_time_monotonic(&self) -> crate::MonotonicInstant {
                crate::MemoryIO::new().current_time_monotonic()
            }
            fn current_time_wall_clock(&self) -> crate::WallClockInstant {
                crate::MemoryIO::new().current_time_wall_clock()
            }
        }

        let io: Arc<dyn IO> = Arc::new(NoTempSpaceIO);
        let pad = std::vec![b'q'; 4096];

        // TempStore::Default falls back to memory and answers cleanly.
        let mut default_sorter = Sorter::new(
            &[SortOrder::Asc],
            std::vec![CollationSeq::Binary],
            std::vec![None],
            std::vec![None],
            2048,
            512,
            io.clone(),
            crate::TempStore::Default,
        )
        .unwrap();
        default_sorter.limit_to(Some(2));
        for k in [30i64, 10, 20] {
            let rec =
                ImmutableRecord::from_values(&[Value::from_i64(k), Value::Blob(pad.clone())], 2)
                    .unwrap();
            io.block(|| default_sorter.insert(&rec)).unwrap();
        }
        io.block(|| default_sorter.sort()).unwrap();
        let mut out = Vec::new();
        while default_sorter.has_more() {
            let vals = default_sorter.record().unwrap().get_values().unwrap();
            out.push(vals[0].as_int().unwrap());
            io.block(|| default_sorter.next()).unwrap();
        }
        assert_eq!(out, std::vec![10, 20]);

        // TempStore::File must still surface the temp-space I/O error.
        let mut file_sorter = Sorter::new(
            &[SortOrder::Asc],
            std::vec![CollationSeq::Binary],
            std::vec![None],
            std::vec![None],
            2048,
            512,
            io.clone(),
            crate::TempStore::File,
        )
        .unwrap();
        file_sorter.limit_to(Some(2));
        let rec = ImmutableRecord::from_values(&[Value::from_i64(1), Value::Blob(pad)], 2).unwrap();
        assert!(
            io.block(|| file_sorter.insert(&rec)).is_err(),
            "explicit TempStore::File must still fail when temp space is unavailable"
        );
    }
}
