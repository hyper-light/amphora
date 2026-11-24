use crate::alloc::vec;
use crate::alloc::*;
use crate::turso_assert;
use crate::types::IOResultOr;
use crate::{
    error::LimboError,
    io::{Buffer, Completion, TempFile, IO},
    io_yield_one, return_if_io,
    storage::sqlite3_ondisk::{read_varint, read_varint_partial, varint_len, write_varint},
    sync::{
        atomic::{self, AtomicUsize},
        Arc, RwLock,
    },
    translate::collate::CollationSeq,
    types::{IOCompletions, IOResult, Value, ValueRef},
    vdbe::metrics::HashJoinMetrics,
    CompletionError, Numeric, Result,
};
use amphora_engine_macros::{turso_assert_eq, AtomicEnum};
use branches::{mark_unlikely, unlikely};
use rapidhash::fast::RapidHasher;
use smallvec::SmallVec;
use std::{cell::RefCell, cmp::Ordering, hash::Hasher};

/// Inline storage for hash join key values (1 key inline avoids heap allocs for single-key joins).
pub(crate) type HashKeyValues = SmallVec<[Value; 1]>;
/// Inline storage for hash join payload values (2 values inline avoids heap allocs for common joins).
pub(crate) type HashPayloadValues = SmallVec<[Value; 2]>;

/// Sentinel index for the end of an intrusive hash bucket chain in `HashTable::heads` / `next`.
const EMPTY_ENTRY: u32 = u32::MAX;

const DEFAULT_SEED: u64 = 1337;

/// Amphora (docs/design/executor.md, decision 8; QT.5): the share of a statement's memory
/// budget (`Pager::statement_memory_budget`) each of its `tables` hash and group tables holds
/// before it spills: equal shares, so together they hold no more than the budget.
pub fn table_share(statement_budget: usize, tables: usize) -> usize {
    statement_budget
        .checked_div(tables)
        .unwrap_or(statement_budget)
}
/// Minimum number of partitions for grace hash join.
pub const MIN_PARTITIONS: usize = 16;
/// Maximum number of partitions for adaptive partitioning.
pub const MAX_PARTITIONS: usize = 128;
const NULL_HASH: u8 = 0;
const INT_HASH: u8 = 1;
const FLOAT_HASH: u8 = 2;
const TEXT_HASH: u8 = 3;
const BLOB_HASH: u8 = 4;

#[inline]
/// Hash text case-insensitively without allocation (ASCII-only for SQLite NOCASE).
/// SQLite's NOCASE collation only considers ASCII case, so to_ascii_lowercase() is correct.
fn hash_text_nocase(hasher: &mut impl Hasher, text: &str) {
    hasher.write_usize(text.len());
    for byte in text.bytes() {
        hasher.write_u8(byte.to_ascii_lowercase());
        if byte == 0 {
            break;
        }
    }
}

/// Hash function for join keys using rapidhash
/// Takes collation into account when hashing text values
pub(crate) fn hash_join_key(key_values: &[ValueRef], collations: &[CollationSeq]) -> u64 {
    let mut hasher = RapidHasher::new(DEFAULT_SEED);
    for (idx, value) in key_values.iter().enumerate() {
        hash_key_part(&mut hasher, idx, value, collations);
    }
    hasher.finish()
}

/// AUDIT 3.1: [hash_join_key] of keys as a probe holds them, without a `ValueRef` copy of them.
fn hash_join_values(key_values: &[Value], collations: &[CollationSeq]) -> u64 {
    let mut hasher = RapidHasher::new(DEFAULT_SEED);
    for (idx, value) in key_values.iter().enumerate() {
        hash_key_part(&mut hasher, idx, &value.as_ref(), collations);
    }
    hasher.finish()
}

#[inline]
fn hash_key_part(
    hasher: &mut RapidHasher,
    idx: usize,
    value: &ValueRef,
    collations: &[CollationSeq],
) {
    match value {
        ValueRef::Null => {
            hasher.write_u8(NULL_HASH);
        }
        ValueRef::Numeric(Numeric::Integer(i)) => {
            // Hash integers in the same bucket as numerically equivalent REALs so e.g. 10 and 10.0 have the same hash.
            let f = *i as f64;
            if (f as i64) == *i && f.is_finite() {
                hasher.write_u8(FLOAT_HASH);
                let bits = normalized_f64_bits(f);
                hasher.write(&bits.to_le_bytes());
            } else {
                // Fallback to the integer domain when the float representation would lose precision.
                hasher.write_u8(INT_HASH);
                hasher.write_i64(*i);
            }
        }
        ValueRef::Numeric(Numeric::Float(f)) => {
            hasher.write_u8(FLOAT_HASH);
            let bits = normalized_f64_bits(f64::from(*f));
            hasher.write(&bits.to_le_bytes());
        }
        ValueRef::Text(text) => {
            let collation = collations.get(idx).unwrap_or(&CollationSeq::Binary);
            hasher.write_u8(TEXT_HASH);
            match *collation {
                CollationSeq::NoCase => {
                    hash_text_nocase(hasher, text.as_str());
                }
                CollationSeq::Rtrim => {
                    let trimmed = text.as_str().trim_end_matches(' ');
                    hasher.write(trimmed.as_bytes());
                }
                CollationSeq::Binary | CollationSeq::Unset => {
                    hasher.write(text.as_bytes());
                }
                CollationSeq::Locale(_) => {
                    hasher.write(&collation.hash_key(text.as_str()));
                }
                CollationSeq::Custom(_) => {
                    unreachable!("custom collations are rejected before hash table construction")
                }
            }
        }
        ValueRef::Blob(blob) => {
            hasher.write_u8(BLOB_HASH);
            hasher.write(blob);
        }
    }
}

/// Normalize signed zero so 0.0 and -0.0 hash the same.
#[inline]
pub(crate) const fn normalized_f64_bits(f: f64) -> u64 {
    if f == 0.0 {
        0.0f64.to_bits()
    } else {
        f.to_bits()
    }
}

/// Check if any of the key values is NULL.
/// Rows with NULL join keys should be skipped in hash joins since NULL != NULL in SQL.
fn has_null_key(key_values: &[Value]) -> bool {
    key_values.iter().any(|v| matches!(v, Value::Null))
}

/// Check if any of the key value refs is NULL.
fn has_null_key_ref(key_values: &[ValueRef]) -> bool {
    key_values.iter().any(|v| matches!(v, ValueRef::Null))
}

/// Check if two key value arrays are equal, taking collation into account.
fn keys_equal(key1: &[Value], key2: &[ValueRef], collations: &[CollationSeq]) -> bool {
    if key1.len() != key2.len() {
        return false;
    }
    for (idx, (v1, v2)) in key1.iter().zip(key2.iter()).enumerate() {
        let collation = collations.get(idx).copied().unwrap_or(CollationSeq::Binary);
        if !values_equal(v1.as_ref(), *v2, collation) {
            return false;
        }
    }
    true
}

/// AUDIT 3.1: `buf` becomes a copy of `keys` in place, its text and blob values reusing their own
/// allocations, so a refill allocates nothing once `buf` has held keys this size.
fn refill_keys<'a>(
    buf: &mut Vec<Value>,
    keys: impl ExactSizeIterator<Item = &'a Value>,
) -> Result<()> {
    let n = keys.len();
    buf.truncate(n);
    buf.try_reserve(
        n.checked_sub(buf.len())
            .ok_or(LimboError::IntegerOverflow)?,
    )?;
    buf.resize(n, Value::Null);
    for (kept, key) in buf.iter_mut().zip(keys) {
        kept.try_clone_from(key)?;
    }
    Ok(())
}

/// [keys_equal] of two keys as they are held, without a `ValueRef` copy of either.
fn keys_equal_values(key1: &[Value], key2: &[Value], collations: &[CollationSeq]) -> bool {
    key1.len() == key2.len()
        && key1.iter().zip(key2).enumerate().all(|(idx, (v1, v2))| {
            let collation = collations.get(idx).copied().unwrap_or(CollationSeq::Binary);
            values_equal(v1.as_ref(), v2.as_ref(), collation)
        })
}

/// Check if two values are equal, using the specified collation for text comparison.
/// NOTE: In SQL, NULL = NULL evaluates to NULL (falsy), so this returns false for NULL comparisons.
fn values_equal(v1: ValueRef, v2: ValueRef, collation: CollationSeq) -> bool {
    match (v1, v2) {
        // NULL = NULL is false in SQL (actually NULL, which is falsy)
        (ValueRef::Null, _) | (_, ValueRef::Null) => false,
        (ValueRef::Numeric(n1), ValueRef::Numeric(n2)) => {
            ValueRef::Numeric(n1) == ValueRef::Numeric(n2)
        }
        (ValueRef::Blob(b1), ValueRef::Blob(b2)) => b1 == b2,
        (ValueRef::Text(t1), ValueRef::Text(t2)) => {
            // Use collation for text comparison
            collation.compare_strings(t1.as_str(), t2.as_str()) == Ordering::Equal
        }
        _ => false,
    }
}

/// Amphora (QT.5 step 4, lane qs5-21): the order a sort-merge join's table sorts its keys in:
/// SQLite's (NULL first, then numbers by value, text by the key's collation, blobs by their
/// bytes), so the keys `keys_equal` calls equal sit together in one run.
fn merge_keys_cmp<'a>(
    a: impl Iterator<Item = ValueRef<'a>>,
    b: impl Iterator<Item = ValueRef<'a>>,
    collations: &[CollationSeq],
) -> Ordering {
    for (idx, (x, y)) in a.zip(b).enumerate() {
        let order = match (x, y) {
            (ValueRef::Text(l), ValueRef::Text(r)) => collations
                .get(idx)
                .copied()
                .unwrap_or(CollationSeq::Binary)
                .compare_strings(l.as_str(), r.as_str()),
            (x, y) => x.cmp(&y),
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    Ordering::Equal
}

/// DISTINCT equality: NULLs compare equal to NULL.
pub(crate) fn values_equal_distinct(v1: ValueRef, v2: ValueRef, collation: CollationSeq) -> bool {
    match (v1, v2) {
        (ValueRef::Null, ValueRef::Null) => true,
        (ValueRef::Null, _) | (_, ValueRef::Null) => false,
        _ => values_equal(v1, v2, collation),
    }
}

pub(crate) fn keys_equal_distinct(
    key1: &[Value],
    key2: &[ValueRef],
    collations: &[CollationSeq],
) -> bool {
    if key1.len() != key2.len() {
        return false;
    }
    for (idx, (v1, v2)) in key1.iter().zip(key2.iter()).enumerate() {
        let collation = collations.get(idx).copied().unwrap_or(CollationSeq::Binary);
        if !values_equal_distinct(v1.as_ref(), *v2, collation) {
            return false;
        }
    }
    true
}

/// State machine states for hash table operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashTableState {
    Building,
    Probing,
    Spilled,
    GraceProcessing,
    Closed,
}

/// A probe entry returned by `grace_next_probe_entry()`.
/// The VDBE writes these to registers for HashProbe to use.
#[derive(Debug)]
pub struct GraceProbeEntry {
    pub key_values: Vec<Value>,
    pub probe_rowid: i64,
}

/// A single entry in a hash table.
#[derive(Debug, Clone)]
pub struct HashEntry {
    /// Hash value of the join keys.
    pub hash: u64,
    /// The join key values (inline for single-key joins to eliminate per-row heap allocs).
    pub key_values: HashKeyValues,
    /// The rowid of the row in the build table.
    /// During probe phase, we'll use SeekRowid to fetch the full row
    /// (unless payload_values contains all needed columns).
    pub rowid: i64,
    /// Optional payload values - columns from the build table that are stored
    /// directly in the hash entry to avoid SeekRowid during probe phase.
    /// When populated, these are the result columns needed from the build table,
    /// stored in column index order as specified during hash table construction.
    pub payload_values: HashPayloadValues,
}

#[derive(Debug)]
pub(crate) struct PendingHashInsert {
    pub(crate) key_values: HashKeyValues,
    pub(crate) rowid: i64,
    pub(crate) payload_values: HashPayloadValues,
}

#[derive(Debug)]
pub(crate) enum HashInsertResult {
    Done,
    IO {
        io: IOCompletions,
        pending: PendingHashInsert,
    },
}

impl HashEntry {
    fn new(hash: u64, key_values: impl Into<HashKeyValues>, rowid: i64) -> Self {
        Self {
            hash,
            key_values: key_values.into(),
            rowid,
            payload_values: HashPayloadValues::new(),
        }
    }

    fn new_with_payload(
        hash: u64,
        key_values: impl Into<HashKeyValues>,
        rowid: i64,
        payload_values: impl Into<HashPayloadValues>,
    ) -> Self {
        Self {
            hash,
            key_values: key_values.into(),
            rowid,
            payload_values: payload_values.into(),
        }
    }

    /// Returns true if this entry has payload values stored.
    pub fn has_payload(&self) -> bool {
        !self.payload_values.is_empty()
    }

    /// Amphora (QT.5, T-QT.5.7): an entry's bytes past its keys and payload as the memory budget
    /// counts them: its hash and its rowid, 8 bytes each. The planner's hash entry is a build row
    /// plus these (`CostModelParams::with_hash_build`).
    pub(crate) const FIXED_BYTES: usize = 8 + 8;

    /// Get the size of this entry in bytes (approximate).
    /// This is a lightweight estimate for memory budgeting, not a precise measurement.
    fn size_bytes(&self) -> usize {
        Self::size_from_values(&self.key_values, &self.payload_values)
    }

    fn size_from_values(key_values: &[Value], payload_values: &[Value]) -> usize {
        let value_size = |v: &Value| match v {
            Value::Null => 1,
            Value::Numeric(_) => 8,
            Value::Text(t) => t.as_str().len(),
            Value::Blob(b) => b.len(),
        };
        let key_size: usize = key_values.iter().map(value_size).sum();
        let payload_size: usize = payload_values.iter().map(value_size).sum();
        // A budgeting estimate: held at usize::MAX, a size too large to count stays over every
        // budget, so the entry spills as one that large would.
        key_size
            .saturating_add(payload_size)
            .saturating_add(Self::FIXED_BYTES)
    }

    /// Calculate the serialized size of a single Value.
    #[inline]
    fn value_serialized_size(v: &Value) -> usize {
        1 + match v {
            Value::Null => 0,
            Value::Numeric(_) => 8,
            Value::Text(t) => {
                let len = t.as_str().len();
                varint_len(len as u64) + len
            }
            Value::Blob(b) => varint_len(b.len() as u64) + b.len(),
        }
    }

    /// Calculate the exact serialized size of this entry.
    fn serialized_size(&self) -> usize {
        8 + 8 // hash + rowid
            + varint_len(self.key_values.len() as u64)
            + self.key_values.iter().map(Self::value_serialized_size).sum::<usize>()
            + varint_len(self.payload_values.len() as u64)
            + self.payload_values.iter().map(Self::value_serialized_size).sum::<usize>()
    }

    /// Serialize this entry directly to a slice, returns bytes written.
    /// The caller must ensure the slice is large enough (use serialized_size()).
    fn serialize_to_slice(&self, buf: &mut [u8]) -> usize {
        let mut offset = 0;

        // Write hash and rowid
        buf[offset..offset + 8].copy_from_slice(&self.hash.to_le_bytes());
        offset += 8;
        buf[offset..offset + 8].copy_from_slice(&self.rowid.to_le_bytes());
        offset += 8;

        // Write number of keys and key values
        offset += write_varint(&mut buf[offset..], self.key_values.len() as u64);
        for value in &self.key_values {
            offset += Self::serialize_value_to_slice(value, &mut buf[offset..]);
        }

        // Write number of payload values and payload values
        offset += write_varint(&mut buf[offset..], self.payload_values.len() as u64);
        for value in &self.payload_values {
            offset += Self::serialize_value_to_slice(value, &mut buf[offset..]);
        }

        offset
    }

    /// Helper to serialize a single Value directly to a slice. Returns bytes written.
    #[inline]
    fn serialize_value_to_slice(value: &Value, buf: &mut [u8]) -> usize {
        let mut offset = 0;
        match value {
            Value::Null => {
                buf[offset] = NULL_HASH;
                offset += 1;
            }
            Value::Numeric(Numeric::Integer(i)) => {
                buf[offset] = INT_HASH;
                offset += 1;
                buf[offset..offset + 8].copy_from_slice(&i.to_le_bytes());
                offset += 8;
            }
            Value::Numeric(Numeric::Float(f)) => {
                buf[offset] = FLOAT_HASH;
                offset += 1;
                buf[offset..offset + 8].copy_from_slice(&f64::from(*f).to_le_bytes());
                offset += 8;
            }
            Value::Text(t) => {
                buf[offset] = TEXT_HASH;
                offset += 1;
                let bytes = t.as_str().as_bytes();
                offset += write_varint(&mut buf[offset..], bytes.len() as u64);
                buf[offset..offset + bytes.len()].copy_from_slice(bytes);
                offset += bytes.len();
            }
            Value::Blob(b) => {
                buf[offset] = BLOB_HASH;
                offset += 1;
                offset += write_varint(&mut buf[offset..], b.len() as u64);
                buf[offset..offset + b.len()].copy_from_slice(b);
                offset += b.len();
            }
        }
        offset
    }

    /// Serialize this entry to bytes for disk storage.
    /// Format: [hash:8][rowid:8][num_keys:varint][keys...][num_payload:varint][payload...]
    /// Each value is: [type:1][len:varint (for text/blob)][data]
    fn serialize(&self, buf: &mut Vec<u8>) -> Result<()> {
        buf.try_reserve(self.serialized_size())?;
        buf.extend_from_slice(&self.hash.to_le_bytes());
        buf.extend_from_slice(&self.rowid.to_le_bytes());

        // Write number of keys and key values
        let varint_buf = &mut [0u8; 9];
        let len = write_varint(varint_buf, self.key_values.len() as u64);
        buf.extend_from_slice(&varint_buf[..len]);
        for value in &self.key_values {
            Self::serialize_value(value, buf, varint_buf);
        }

        // Write number of payload values and payload values
        let len = write_varint(varint_buf, self.payload_values.len() as u64);
        buf.extend_from_slice(&varint_buf[..len]);
        for value in &self.payload_values {
            Self::serialize_value(value, buf, varint_buf);
        }
        Ok(())
    }

    /// Helper to serialize a single Value to bytes.
    fn serialize_value(value: &Value, buf: &mut Vec<u8>, varint_buf: &mut [u8; 9]) {
        match value {
            Value::Null => {
                buf.push(NULL_HASH);
            }
            Value::Numeric(Numeric::Integer(i)) => {
                buf.push(INT_HASH);
                buf.extend_from_slice(&i.to_le_bytes());
            }
            Value::Numeric(Numeric::Float(f)) => {
                buf.push(FLOAT_HASH);
                buf.extend_from_slice(&f64::from(*f).to_le_bytes());
            }
            Value::Text(t) => {
                buf.push(TEXT_HASH);
                let bytes = t.as_str().as_bytes();
                let len = write_varint(varint_buf, bytes.len() as u64);
                buf.extend_from_slice(&varint_buf[..len]);
                buf.extend_from_slice(bytes);
            }
            Value::Blob(b) => {
                buf.push(BLOB_HASH);
                let len = write_varint(varint_buf, b.len() as u64);
                buf.extend_from_slice(&varint_buf[..len]);
                buf.extend_from_slice(b);
            }
        }
    }

    /// Deserialize an entry from bytes, returning (entry, bytes_consumed) or error.
    fn deserialize(buf: &[u8]) -> Result<(Self, usize)> {
        if unlikely(buf.len() < 16) {
            return Err(LimboError::Corrupt(
                "HashEntry: buffer too small for header".to_string(),
            ));
        }

        // buffer len checked above
        let hash = u64::from_le_bytes(buf[0..8].try_into().expect("expect 8 bytes"));
        let rowid = i64::from_le_bytes(buf[8..16].try_into().expect("expect 8 bytes"));
        let mut offset = 16;

        // Read number of keys and key values
        let (num_keys, varint_len) = read_varint(&buf[offset..])?;
        offset += varint_len;

        let mut key_values = HashKeyValues::new();
        let num_keys_usize = usize::try_from(num_keys).map_err(|_| LimboError::IntegerOverflow)?;
        if num_keys_usize > key_values.inline_size() {
            key_values
                .try_reserve_exact(num_keys_usize)
                .map_err(|_| LimboError::OutOfMemory)?;
        }
        for _ in 0..num_keys_usize {
            let (value, consumed) = Self::deserialize_value(&buf[offset..])?;
            key_values.push(value);
            offset += consumed;
        }

        // Read number of payload values and payload values
        let (num_payload, varint_len) = read_varint(&buf[offset..])?;
        offset += varint_len;

        let mut payload_values = HashPayloadValues::new();
        let num_payload_usize =
            usize::try_from(num_payload).map_err(|_| LimboError::IntegerOverflow)?;
        if num_payload_usize > payload_values.inline_size() {
            payload_values
                .try_reserve_exact(num_payload_usize)
                .map_err(|_| LimboError::OutOfMemory)?;
        }
        for _ in 0..num_payload_usize {
            let (value, consumed) = Self::deserialize_value(&buf[offset..])?;
            payload_values.push(value);
            offset += consumed;
        }

        Ok((
            Self {
                hash,
                key_values,
                rowid,
                payload_values,
            },
            offset,
        ))
    }

    /// Helper to deserialize a single Value from bytes.
    /// Returns (Value, bytes_consumed).
    fn deserialize_value(buf: &[u8]) -> Result<(Value, usize)> {
        if unlikely(buf.is_empty()) {
            return Err(LimboError::Corrupt(
                "HashEntry: unexpected end of buffer".to_string(),
            ));
        }
        let value_type = buf[0];
        let mut offset = 1;

        let value = match value_type {
            NULL_HASH => Value::Null,
            INT_HASH => {
                if unlikely(offset + 8 > buf.len()) {
                    return Err(LimboError::Corrupt(
                        "HashEntry: buffer too small for integer".to_string(),
                    ));
                }
                let i =
                    i64::from_le_bytes(buf[offset..offset + 8].try_into().expect("expect 8 bytes"));
                offset += 8;
                Value::from_i64(i)
            }
            FLOAT_HASH => {
                if unlikely(offset + 8 > buf.len()) {
                    return Err(LimboError::Corrupt(
                        "HashEntry: buffer too small for float".to_string(),
                    ));
                }
                let f =
                    f64::from_le_bytes(buf[offset..offset + 8].try_into().expect("expect 8 bytes"));
                offset += 8;
                Value::from_f64(f)
            }
            TEXT_HASH => {
                let (str_len, varint_len) = read_varint(&buf[offset..])?;
                offset += varint_len;
                if unlikely(offset + str_len as usize > buf.len()) {
                    return Err(LimboError::Corrupt(
                        "HashEntry: buffer too small for text".to_string(),
                    ));
                }
                // SAFETY: We serialized this data ourselves, so it should be valid UTF-8.
                // Skipping validation here for performance in the spill/reload path.
                // Doing checked utf8 construction here is a massive performance hit.
                let bytes = buf[offset..offset + str_len as usize].to_vec();
                let s = unsafe { String::from_utf8_unchecked(bytes) };
                offset += str_len as usize;
                Value::Text(s.into())
            }
            BLOB_HASH => {
                let (blob_len, varint_len) = read_varint(&buf[offset..])?;
                offset += varint_len;
                if unlikely(offset + blob_len as usize > buf.len()) {
                    return Err(LimboError::Corrupt(
                        "HashEntry: buffer too small for blob".to_string(),
                    ));
                }
                let b =
                    crate::types::value_blob_from_slice(&buf[offset..offset + blob_len as usize])?;
                offset += blob_len as usize;
                Value::Blob(b)
            }
            _ => {
                mark_unlikely();
                return Err(LimboError::Corrupt(format!(
                    "HashEntry: unknown value type {value_type}",
                )));
            }
        };
        Ok((value, offset))
    }
}

#[derive(Debug, Clone, Copy)]
struct Partitioning {
    count: usize,
    mask: usize,
    shift: u32,
}

impl Partitioning {
    fn new(count: usize) -> Self {
        turso_assert!(
            count.is_power_of_two(),
            "partition count must be a power of two"
        );
        let bits = count.trailing_zeros();
        Self {
            count,
            mask: count - 1,
            shift: 64 - bits,
        }
    }

    #[inline(always)]
    fn index(&self, hash: u64) -> usize {
        ((hash >> self.shift) as usize) & self.mask
    }
}

/// A bucket in the hash table. Uses chaining for collision resolution.
#[derive(Debug, Clone)]
pub struct HashBucket {
    entries: Vec<HashEntry>,
}

impl HashBucket {
    fn new() -> Self {
        Self { entries: vec![] }
    }

    fn insert(&mut self, entry: HashEntry) -> Result<()> {
        self.entries.try_push(entry)?;
        Ok(())
    }

    const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn size_bytes(&self) -> usize {
        self.entries.iter().map(|e| e.size_bytes()).sum()
    }
}

/// I/O state for spilled partition operations
#[derive(Debug, AtomicEnum, Clone, Copy, PartialEq, Eq)]
pub enum SpillIOState {
    None,
    WaitingForWrite,
    WriteComplete,
    WaitingForRead,
    ReadComplete,
    Error,
}

/// State of a partition in a spilled hash table
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionState {
    /// data is in partition_buffers
    InMemory,
    /// Has been written to disk, not yet loaded
    OnDisk,
    /// Is being loaded from disk (I/O in progress)
    Loading,
    /// Has been loaded from disk and is ready for probing
    Loaded,
}

/// A chunk of partition data spilled to disk.
/// A partition may be spilled multiple times, creating multiple chunks.
#[derive(Debug)]
struct SpillChunk {
    /// File offset where this chunk's data starts
    file_offset: u64,
    /// Size in bytes of this chunk on disk
    size_bytes: usize,
    /// Number of entries in this chunk
    num_entries: usize,
}

/// Tracks a partition that has been spilled to disk during grace hash join.
pub struct SpilledPartition {
    /// Partition index (0 to partition_count - 1)
    pub partition_idx: usize,
    /// Chunks of data belonging to this partition (may have multiple spills)
    chunks: Vec<SpillChunk>,
    /// Current state of the partition
    state: PartitionState,
    /// I/O state for async operations
    io_state: Arc<AtomicSpillIOState>,
    /// Read buffer for loading partition back
    read_buffer: Arc<RwLock<Vec<u8>>>,
    /// Length of data in read buffer
    buffer_len: Arc<AtomicUsize>,
    /// Hash buckets for this partition (populated after loading)
    buckets: Vec<HashBucket>,
    /// Current chunk being loaded (for multi-chunk reads)
    current_chunk_idx: usize,
    /// Approximate memory used by the resident buckets for this partition
    resident_mem: usize,
    /// Parallel to `buckets`: tracks which entries have been matched (for FULL OUTER JOIN).
    matched_bits: Vec<Vec<bool>>,
    /// Partial entry bytes spanning chunk boundaries
    partial_entry: Vec<u8>,
    /// Parsed entries for validation
    parsed_entries: usize,
}

impl SpilledPartition {
    fn new(partition_idx: usize) -> Self {
        Self {
            partition_idx,
            chunks: vec![],
            state: PartitionState::OnDisk,
            io_state: Arc::new(AtomicSpillIOState::new(SpillIOState::None)),
            read_buffer: Arc::new(RwLock::new(vec![])),
            buffer_len: Arc::new(atomic::AtomicUsize::new(0)),
            buckets: vec![],
            current_chunk_idx: 0,
            resident_mem: 0,
            matched_bits: vec![],
            partial_entry: vec![],
            parsed_entries: 0,
        }
    }

    /// Add a new chunk of data to this partition
    fn add_chunk(&mut self, file_offset: u64, size_bytes: usize, num_entries: usize) -> Result<()> {
        self.chunks.try_push(SpillChunk {
            file_offset,
            size_bytes,
            num_entries,
        })?;
        Ok(())
    }

    /// Get total size in bytes across all chunks
    fn total_size_bytes(&self) -> usize {
        self.chunks.iter().map(|c| c.size_bytes).sum()
    }

    /// Get total number of entries across all chunks
    fn total_num_entries(&self) -> usize {
        self.chunks.iter().map(|c| c.num_entries).sum()
    }

    fn buffer_len(&self) -> usize {
        self.buffer_len.load(atomic::Ordering::Acquire)
    }

    /// Check if partition is ready for probing
    pub const fn is_loaded(&self) -> bool {
        matches!(
            self.state,
            PartitionState::Loaded | PartitionState::InMemory
        )
    }

    /// Check if there are more chunks to load
    const fn has_more_chunks(&self) -> bool {
        self.current_chunk_idx < self.chunks.len()
    }

    /// Get the current chunk to load, if any
    fn current_chunk(&self) -> Option<&SpillChunk> {
        self.chunks.get(self.current_chunk_idx)
    }
}

/// In-memory partition buffer for grace hash join.
/// During build phase, entries are first accumulated here before spilling.
struct PartitionBuffer {
    /// Entries in this partition
    entries: Vec<HashEntry>,
    /// Total memory used by entries in this partition
    mem_used: usize,
}

impl PartitionBuffer {
    fn new() -> Self {
        Self {
            entries: vec![],
            mem_used: 0,
        }
    }

    fn insert(&mut self, entry: HashEntry) -> Result<()> {
        self.mem_used += entry.size_bytes();
        self.entries.try_push(entry)?;
        Ok(())
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.mem_used = 0;
    }

    const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Configuration for the hash table.
#[derive(Debug, Clone)]
pub struct HashTableConfig {
    /// Initial number of buckets (must be power of 2).
    pub initial_buckets: usize,
    /// Maximum memory budget in bytes.
    pub mem_budget: usize,
    /// Number of keys in the join condition.
    pub num_keys: usize,
    /// Collation sequences for each join key.
    pub collations: Vec<CollationSeq>,
    /// Only spill to a file when != TempStore::Memory
    pub temp_store: crate::TempStore,
    /// Whether to track which entries have been matched during probing (for FULL OUTER JOIN).
    pub track_matched: bool,
    /// Optional override for the number of partitions (must be power of two).
    pub partition_count: Option<usize>,
}

/// Tests' tables: a budget of 32 KiB, small enough that a few hundred rows spill.
#[cfg(test)]
impl Default for HashTableConfig {
    fn default() -> Self {
        Self {
            initial_buckets: 1024,
            mem_budget: 32 * 1024,
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Default,
            track_matched: false,
            partition_count: None,
        }
    }
}

struct SpillState {
    /// In-memory partition buffers for grace hash join.
    /// When spilling is triggered, entries are partitioned by hash before writing.
    partition_buffers: Vec<PartitionBuffer>,
    /// Spilled partitions metadata, tracks what's on disk
    partitions: Vec<SpilledPartition>,
    /// Current file offset for next spill write
    next_spill_offset: u64,
    /// Temporary file for spilled data.
    temp_file: TempFile,
    /// Partitioning strategy for this spill.
    partitioning: Partitioning,
}

impl SpillState {
    fn new(
        io: &Arc<dyn IO>,
        temp_store: crate::TempStore,
        partitioning: Partitioning,
    ) -> Result<Self> {
        Ok(SpillState {
            partition_buffers: (0..partitioning.count)
                .map(|_| PartitionBuffer::new())
                .try_collect()?,
            partitions: vec![],
            next_spill_offset: 0,
            temp_file: TempFile::with_temp_store(io, temp_store)?,
            partitioning,
        })
    }

    fn find_partition_mut(&mut self, logical_idx: usize) -> Option<&mut SpilledPartition> {
        self.partitions
            .iter_mut()
            .find(|p| p.partition_idx == logical_idx)
    }

    fn find_partition(&self, logical_idx: usize) -> Option<&SpilledPartition> {
        self.partitions
            .iter()
            .find(|p| p.partition_idx == logical_idx)
    }
}

/// Probe-side buffering/spilling state for grace hash join.
/// Reuses the same serialization format and types as build-side spilling.
struct ProbeSpillState {
    /// In-memory partition buffers for probe rows targeting spilled build partitions.
    partition_buffers: Vec<PartitionBuffer>,
    /// Spilled probe partition metadata.
    partitions: Vec<SpilledPartition>,
    /// Current file offset for next probe spill write.
    next_spill_offset: u64,
    /// Separate temp file for probe-side spills.
    temp_file: TempFile,
    /// Same partitioning as build side, so partition indices correspond.
    partitioning: Partitioning,
    /// Current memory used by probe buffers.
    mem_used: usize,
    /// Memory budget for probe-side buffers.
    mem_budget: usize,
}

impl ProbeSpillState {
    fn new(
        io: &Arc<dyn IO>,
        temp_store: crate::TempStore,
        partitioning: Partitioning,
        mem_budget: usize,
    ) -> Result<Self> {
        Ok(Self {
            partition_buffers: (0..partitioning.count)
                .map(|_| PartitionBuffer::new())
                .try_collect()?,
            partitions: vec![],
            next_spill_offset: 0,
            temp_file: TempFile::with_temp_store(io, temp_store)?,
            partitioning,
            mem_used: 0,
            mem_budget,
        })
    }

    fn find_partition_mut(&mut self, logical_idx: usize) -> Option<&mut SpilledPartition> {
        self.partitions
            .iter_mut()
            .find(|p| p.partition_idx == logical_idx)
    }

    fn find_partition(&self, logical_idx: usize) -> Option<&SpilledPartition> {
        self.partitions
            .iter()
            .find(|p| p.partition_idx == logical_idx)
    }
}

/// State for grace hash join partition-by-partition processing.
/// The VDBE drives the loop; this tracks partition iteration and probe chunk loading.
struct GraceState {
    /// Current probe entries loaded from disk (one chunk at a time).
    probe_entries: Vec<HashEntry>,
    /// Cursor into probe_entries.
    probe_entry_cursor: usize,
    /// Ordered list of partition indices to process (only spilled ones).
    partitions_to_process: Vec<usize>,
    /// Index into partitions_to_process.
    partition_list_idx: usize,
    /// Current load state for the active grace partition.
    load_state: GracePartitionLoadState,
}

impl GraceState {
    fn current_partition_idx(&self) -> Option<usize> {
        self.partitions_to_process
            .get(self.partition_list_idx)
            .copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GracePartitionLoadState {
    NeedBuildLoad,
    NeedProbeLoad,
    Ready,
}

/// HashTable is the build-side data structure used for hash joins and DISTINCT. It behaves like a
/// standard in-memory hash table until a configurable memory budget is exceeded, at
/// which point it transparently switches to a grace-hash-join style layout and spills
/// partitions to disk.
///
/// # Overview
///
/// The table is keyed by an N-column join key. Keys are hashed using a stable
/// rapidhash hasher that is aware of SQLite-style collations for text values.
/// Each entry stores:
///
/// - the precomputed hash value,
/// - a owned copy of the join key values, and
/// - the rowid of the build-side row, used later to SeekRowid into the build table.
///
/// Collisions within a hash bucket are resolved using simple chaining (a `Vec<HashEntry>`),
/// and equality is determined by comparing the stored key values against probe keys using
/// the same collation-aware comparison logic that was used when hashing.
pub struct HashTable {
    /// Initial bucket count used to reinitialize after spills.
    initial_buckets: usize,
    /// Power-of-two bucket head indices into `entries` (`EMPTY_ENTRY` when empty), doubled as
    /// entries arrive (AUDIT 3.1, `grow_buckets`). Uses 4 bytes per bucket and zero per-bucket
    /// heap allocations.
    heads: Vec<u32>,
    /// Flat contiguous slab of in-memory entries (used when not spilled).
    entries: Vec<HashEntry>,
    /// Intrusive singly-linked chain pointers parallel to `entries` (`EMPTY_ENTRY` at chain end).
    next: Vec<u32>,
    /// Bytes of the bucket head array past its initial size, counted against `mem_budget` with
    /// `mem_used` so growing never passes the table's memory bound (AUDIT 3.1).
    bucket_mem: usize,
    /// Number of entries in the table.
    num_entries: usize,
    /// Current memory usage in bytes.
    mem_used: usize,
    /// Memory budget in bytes.
    mem_budget: usize,
    /// Number of join keys.
    num_keys: usize,
    /// Collation sequences for each join key.
    collations: Vec<CollationSeq>,
    /// Current state of the hash table.
    state: HashTableState,
    /// IO object for disk operations.
    io: Arc<dyn IO>,
    /// Current probe position bucket index (spilled partitions).
    probe_bucket_idx: usize,
    /// Current probe entry index within bucket (spilled partitions).
    probe_entry_idx: usize,
    /// Last matched entry index in `entries` (non-spilled mode, for `mark_current_matched`).
    probe_current_entry: Option<u32>,
    /// Next entry index in `entries` chain to inspect on `next_match` (non-spilled mode).
    probe_next_entry: u32,
    /// Cached hash of current probe keys (to avoid recomputing)
    current_probe_hash: Option<u64>,
    /// Current probe key values being searched.
    current_probe_keys: Option<Vec<Value>>,
    spill_state: Option<SpillState>,
    /// Index of current spilled partition being probed
    current_spill_partition_idx: usize,
    /// Track non-empty bucket indices in `heads` for fast clear in distinct/group-by usage.
    non_empty_buckets: Vec<usize>,
    /// LRU of resident spilled partitions to cap memory for DISTINCT, grace,
    /// and unmatched-scan partition loads.
    loaded_partitions_lru: RefCell<VecDeque<usize>>,
    /// Memory used by resident (loaded or in-memory) partitions
    loaded_partitions_mem: usize,
    /// Temp storage mode (memory vs file) for spilled data
    temp_store: crate::TempStore,
    /// Whether to track matched entries (for FULL OUTER JOIN).
    track_matched: bool,
    /// Parallel to `entries`: tracks which entries were matched in non-spilled mode.
    matched_bits: Vec<bool>,
    /// Bucket index for iterating unmatched entries (spilled partitions).
    unmatched_scan_bucket: usize,
    /// Entry index within bucket / `entries` for iterating unmatched entries.
    unmatched_scan_entry: usize,
    /// Partition index for iterating unmatched entries in spilled mode.
    unmatched_scan_partition: usize,
    /// Optional override for partition count selection
    partition_count_override: Option<usize>,
    /// Probe-side spill state for grace hash join.
    probe_spill_state: Option<ProbeSpillState>,
    /// Grace processing state machine.
    grace_state: Option<GraceState>,
    /// Amphora (QT.5 step 4, lane qs5-21): a sort-merge join's table (`set_merge`).
    merge: bool,
    /// A merge's order: indices into `entries` sorted by key once the build is done. Empty when
    /// the table isn't a merge's, or spilled or past its budget (then probed by hash).
    merge_order: Vec<u32>,
    /// Where the last probe's run starts in `merge_order`: a probe key at or past the last merges
    /// forward from here, and a repeated key replays its run.
    merge_run: usize,
    /// The position in `merge_order` that `next_match` tests next.
    merge_next: usize,
}

crate::assert::assert_send!(HashTable);

enum SpillAction {
    AlreadyLoaded,
    ParseChunk {
        partition_idx: usize,
    },
    WaitingForIO,
    NoChunks,
    LoadChunk {
        read_size: usize,
        file_offset: u64,
        io_state: Arc<AtomicSpillIOState>,
        buffer_len: Arc<AtomicUsize>,
        read_buffer_ref: Arc<RwLock<Vec<u8>>>,
    },
    Restart,
    NotFound,
}

enum GraceProbeChunkAction {
    WaitingForIO,
    ParseChunk {
        partition_idx: usize,
    },
    LoadChunk {
        read_size: usize,
        file_offset: u64,
        io_state: Arc<AtomicSpillIOState>,
        buffer_len: Arc<AtomicUsize>,
        read_buffer_ref: Arc<RwLock<Vec<u8>>>,
    },
    Restart,
    NoMoreChunks,
}

enum ParseChunkResult {
    MoreChunks,
    Done { resident_mem: usize },
}

impl HashTable {
    /// Create a new hash table.
    pub fn new(config: HashTableConfig, io: Arc<dyn IO>) -> Result<Self> {
        if config
            .collations
            .iter()
            .any(|collation| collation.is_custom())
        {
            // Custom collation equality is a connection-owned callback. Hash tables do
            // not carry connection context, so hash/equality cannot be made consistent here.
            return Err(LimboError::InternalError(
                "custom collations are not supported by hash tables".to_string(),
            ));
        }
        // A power of two, so a bucket is the hash's low bits and doubling splits each chain in two.
        let num_buckets = config.initial_buckets.max(1).next_power_of_two();
        let mut heads = Vec::new();
        heads.try_reserve_exact(num_buckets)?;
        heads.resize(num_buckets, EMPTY_ENTRY);
        Ok(Self {
            initial_buckets: num_buckets,
            heads,
            entries: vec![],
            next: vec![],
            bucket_mem: 0,
            num_entries: 0,
            mem_used: 0,
            mem_budget: config.mem_budget,
            num_keys: config.num_keys,
            collations: config.collations,
            state: HashTableState::Building,
            io,
            probe_bucket_idx: 0,
            probe_entry_idx: 0,
            probe_current_entry: None,
            probe_next_entry: EMPTY_ENTRY,
            current_probe_keys: None,
            current_probe_hash: None,
            spill_state: None,
            current_spill_partition_idx: 0,
            loaded_partitions_lru: VecDeque::new().into(),
            loaded_partitions_mem: 0,
            non_empty_buckets: vec![],
            temp_store: config.temp_store,
            track_matched: config.track_matched,
            matched_bits: vec![],
            unmatched_scan_bucket: 0,
            unmatched_scan_entry: 0,
            unmatched_scan_partition: 0,
            partition_count_override: config.partition_count,
            probe_spill_state: None,
            grace_state: None,
            merge: false,
            merge_order: vec![],
            merge_run: 0,
            merge_next: 0,
        })
    }

    /// Amphora (QT.5 step 4, lane qs5-21): makes this a sort-merge join's table
    /// (`HashJoinMethod::Merge`). Its rows are put in by hash as any table's, so a spill, the
    /// matched bits and the unmatched-row scan stay the hash table's; once the build is done
    /// (`finalize_build`) the rows are sorted by key, and `probe` and `next_match` find a probe
    /// row's matches by merging forward from the previous probe row's run. A table that spilled,
    /// or whose order would pass its budget, is probed by hash instead, counted in
    /// `HashJoinMetrics::merge_tables_hashed`.
    pub fn set_merge(&mut self, merge: bool) {
        self.merge = merge;
    }

    /// Sorts the entries' indices by key into `merge_order`, within the table's budget: false,
    /// and nothing held, when the positions would pass it.
    fn build_merge_order(&mut self) -> Result<bool> {
        let bytes = self
            .num_entries
            .saturating_mul(std::mem::size_of::<u32>());
        if self
            .mem_used
            .saturating_add(self.bucket_mem)
            .saturating_add(bytes)
            > self.mem_budget
        {
            return Ok(false);
        }
        let n = u32::try_from(self.entries.len()).map_err(|_| LimboError::IntegerOverflow)?;
        let mut order: Vec<u32> = Vec::new();
        order.try_reserve_exact(self.entries.len())?;
        order.extend(0..n);
        let entries = &self.entries;
        let collations = &self.collations;
        let entry_at = |idx: u32| usize::try_from(idx).ok().and_then(|i| entries.get(i));
        // In place, with no buffer: the order is all the merge holds beyond the hash table's.
        order.sort_unstable_by(|a, b| match (entry_at(*a), entry_at(*b)) {
            (Some(a), Some(b)) => merge_keys_cmp(
                a.key_values.iter().map(|v| v.as_ref()),
                b.key_values.iter().map(|v| v.as_ref()),
                collations,
            ),
            _ => Ordering::Equal,
        });
        self.mem_used = self.mem_used.saturating_add(bytes);
        self.merge_order = order;
        Ok(true)
    }

    /// A merge's probe: the first entry of the probe key's run. A key at or past the last probe's
    /// merges forward from that run's start; one before it finds its run by binary search.
    fn merge_probe(&mut self, metrics: Option<&mut HashJoinMetrics>) -> Result<Option<&HashEntry>> {
        let (pos, steps, searched) = {
            let Some(probe_keys) = self.current_probe_keys.as_ref() else {
                return Ok(None);
            };
            let entries = &self.entries;
            let collations = &self.collations;
            let order = &self.merge_order;
            let below_probe = |&entry_idx: &u32| {
                usize::try_from(entry_idx)
                    .ok()
                    .and_then(|i| entries.get(i))
                    .is_some_and(|entry| {
                        merge_keys_cmp(
                            entry.key_values.iter().map(|v| v.as_ref()),
                            probe_keys.iter().map(|v| v.as_ref()),
                            collations,
                        ) == Ordering::Less
                    })
            };
            let start = self.merge_run.min(order.len());
            // Every entry before the last run is below the last probe key: the new key is at or
            // past it when the entry just before that run is below the new key too.
            let forward = start
                .checked_sub(1)
                .and_then(|before| order.get(before))
                .is_none_or(below_probe);
            if forward {
                let mut pos = start;
                while order.get(pos).is_some_and(below_probe) {
                    pos = pos.checked_add(1).ok_or(LimboError::IntegerOverflow)?;
                }
                let steps = pos.checked_sub(start).ok_or(LimboError::IntegerOverflow)?;
                (pos, u64::try_from(steps).unwrap_or(u64::MAX), false)
            } else {
                (
                    order.partition_point(|position| below_probe(position)),
                    0,
                    true,
                )
            }
        };
        if let Some(metrics) = metrics {
            metrics.merge_steps = metrics.merge_steps.saturating_add(steps);
            metrics.merge_searches = metrics.merge_searches.saturating_add(u64::from(searched));
        }
        self.merge_run = pos;
        self.merge_next = pos;
        self.merge_next_match()
    }

    /// A merge's next match: the run's next entry, while its key is the probe key's.
    fn merge_next_match(&mut self) -> Result<Option<&HashEntry>> {
        let Some(probe_keys) = self.current_probe_keys.as_deref() else {
            return Ok(None);
        };
        let Some(&entry_idx) = self.merge_order.get(self.merge_next) else {
            return Ok(None);
        };
        let entry_pos = usize::try_from(entry_idx).map_err(|_| LimboError::IntegerOverflow)?;
        let matches = self.entries.get(entry_pos).is_some_and(|entry| {
            keys_equal_values(&entry.key_values, probe_keys, &self.collations)
        });
        if !matches {
            return Ok(None);
        }
        self.merge_next = self
            .merge_next
            .checked_add(1)
            .ok_or(LimboError::IntegerOverflow)?;
        self.probe_current_entry = Some(entry_idx);
        Ok(self.entries.get(entry_pos))
    }

    /// Get the current state of the hash table.
    pub fn get_state(&self) -> &HashTableState {
        &self.state
    }

    /// Based on average entry size and number of entries,
    /// determine the number of partitions to use for spilling.
    fn choose_partition_count(&self, entry_size: usize) -> usize {
        if let Some(count) = self.partition_count_override {
            turso_assert!(
                count.is_power_of_two(),
                "partition count override must be a power of two"
            );
            return count;
        }

        let avg_entry_size = self
            .mem_used
            .checked_div(self.num_entries)
            .map_or_else(|| entry_size.max(1), |avg| avg.max(entry_size));
        let target_partition_bytes = (self.mem_budget / 2).max(avg_entry_size);
        let target_entries_per_partition = (target_partition_bytes / avg_entry_size).max(1);
        let estimated_total_entries = self.num_entries.saturating_add(1);
        let mut partitions = estimated_total_entries.div_ceil(target_entries_per_partition);
        partitions = partitions.clamp(MIN_PARTITIONS, MAX_PARTITIONS);
        partitions.next_power_of_two()
    }

    /// For a given hash value, get the partition index.
    /// SAFETY: only call this when spill_state is Some.
    fn partition_index(&self, hash: u64) -> usize {
        let spill_state = self.spill_state.as_ref().expect("spill state must exist");
        spill_state.partitioning.index(hash)
    }

    fn record_probe_call(&mut self, metrics: Option<&mut HashJoinMetrics>) {
        if let Some(metrics) = metrics {
            metrics.probe_calls = metrics.probe_calls.saturating_add(1);
        }
    }

    /// Insert a row into the hash table, returns IOResult because this may spill to disk.
    /// When memory budget is exceeded, triggers grace hash join by partitioning and spilling.
    /// Rows with NULL join keys are skipped since NULL != NULL in SQL.
    /// (This is specific to hash join semantics, not DISTINCT.)
    pub fn insert(
        &mut self,
        key_values: Vec<Value>,
        rowid: i64,
        payload_values: Vec<Value>,
        metrics: Option<&mut HashJoinMetrics>,
    ) -> IOResultOr<()> {
        let pending = PendingHashInsert {
            key_values: key_values.into(),
            rowid,
            payload_values: payload_values.into(),
        };
        match self.insert_pending(pending, metrics)? {
            HashInsertResult::Done => Ok(IOResult::Done(())),
            HashInsertResult::IO { io, .. } => Ok(IOResult::IO(io)),
        }
    }

    /// Push `entry` into the flat slab and prepend it to bucket `bucket_idx`'s intrusive chain.
    #[inline]
    fn push_slab_entry(&mut self, bucket_idx: usize, entry: HashEntry) -> Result<()> {
        let entry_idx = u32::try_from(self.entries.len()).map_err(|_| LimboError::IntegerOverflow)?;
        let Some(head) = self.heads.get_mut(bucket_idx) else {
            return Err(LimboError::InternalError("hash bucket out of range".into()));
        };
        let prev_head = *head;
        if prev_head == EMPTY_ENTRY {
            self.non_empty_buckets.try_push(bucket_idx)?;
        }
        self.entries.try_push(entry)?;
        self.next.try_push(prev_head)?;
        if self.track_matched {
            self.matched_bits.try_push(false)?;
        }
        if let Some(slot) = self.heads.get_mut(bucket_idx) {
            *slot = entry_idx;
        }
        Ok(())
    }

    pub(crate) fn insert_pending(
        &mut self,
        pending: PendingHashInsert,
        metrics: Option<&mut HashJoinMetrics>,
    ) -> Result<HashInsertResult> {
        turso_assert!(
            matches!(
                self.state,
                HashTableState::Building | HashTableState::Spilled
            ),
            "Cannot insert into hash table in unexpected state",
            { "state": format!("{:?}", self.state) }
        );

        // Skip rows with NULL join keys - they can never match anything since NULL != NULL in SQL.
        // However, when track_matched is enabled (outer joins), we must keep NULL-key entries
        // so they appear as unmatched in the unmatched scan.
        if has_null_key(&pending.key_values) && !self.track_matched {
            return Ok(HashInsertResult::Done);
        }

        // Compute hash of the join keys directly without allocating a temporary `Vec<ValueRef>`.
        let hash = hash_join_values(&pending.key_values, &self.collations);
        let entry_size = HashEntry::size_from_values(&pending.key_values, &pending.payload_values);

        // Check if we would exceed memory budget
        if self.over_budget(entry_size) {
            if self.spill_state.is_none() {
                tracing::debug!(
                    "Hash table memory budget exceeded (used: {}, budget: {}), spilling to disk",
                    self.mem_used,
                    self.mem_budget
                );
                // First time exceeding budget, trigger spill
                // Move all existing slab entries into partition buffers
                let partition_count = self.choose_partition_count(entry_size);
                let partitioning = Partitioning::new(partition_count);
                self.spill_state = Some(SpillState::new(&self.io, self.temp_store, partitioning)?);
                self.redistribute_to_partitions()?;
                self.state = HashTableState::Spilled;
            };

            // Spill whole partitions until the new entry fits
            if let Some(c) = self.spill_partitions_for_entry(entry_size, metrics)? {
                // I/O pending, caller will re-enter after completion and retry the insert.
                if !c.finished() {
                    return Ok(HashInsertResult::IO {
                        io: IOCompletions(c),
                        pending,
                    });
                }
            }
        }

        let PendingHashInsert {
            key_values,
            rowid,
            payload_values,
        } = pending;
        let entry = if payload_values.is_empty() {
            HashEntry::new(hash, key_values, rowid)
        } else {
            HashEntry::new_with_payload(hash, key_values, rowid, payload_values)
        };

        if let Some(spill_state) = self.spill_state.as_mut() {
            let partition_idx = spill_state.partitioning.index(hash);
            // In spilled mode, insert into partition buffer
            spill_state.partition_buffers[partition_idx].insert(entry)?;
        } else {
            // Normal mode, insert into flat slab and bucket chain
            self.grow_buckets(entry_size)?;
            let bucket_idx = Self::bucket_of(hash, self.heads.len());
            self.push_slab_entry(bucket_idx, entry)?;
        }

        self.num_entries += 1;
        self.mem_used += entry_size;

        Ok(HashInsertResult::Done)
    }

    /// Insert keys into the hash table if not already present.
    /// Returns true if inserted, false if duplicate found.
    /// Unlike hash join inserts, DISTINCT keeps NULLs and treats NULL==NULL.
    pub fn insert_distinct(
        &mut self,
        key_values: &[Value],
        key_refs: &[ValueRef],
        mut metrics: Option<&mut HashJoinMetrics>,
    ) -> IOResultOr<bool> {
        turso_assert!(
            self.state == HashTableState::Building || self.state == HashTableState::Spilled,
            "Cannot insert_distinct into hash table in unexpected state",
            { "state": format!("{:?}", self.state) }
        );

        let hash = hash_join_key(key_refs, &self.collations);

        if self.spill_state.is_some() {
            let partition_idx = self.partition_index(hash);
            // Check partition buffer for duplicates
            let has_buffer_dup = {
                let spill_state = self.spill_state.as_ref().expect("spill state exists");
                let buffer = &spill_state.partition_buffers[partition_idx];
                buffer.entries.iter().any(|entry| {
                    entry.hash == hash
                        && keys_equal_distinct(&entry.key_values, key_refs, &self.collations)
                })
            };
            if has_buffer_dup {
                return Ok(IOResult::Done(false));
            }

            // Ensure spilled partition is loaded before checking
            let has_partition = {
                let spill_state = self.spill_state.as_ref().expect("spill state exists");
                spill_state.find_partition(partition_idx).is_some()
            };
            if has_partition && !self.is_partition_loaded(partition_idx) {
                return_if_io!(self.load_spilled_partition(partition_idx, metrics.as_deref_mut()));
            }

            // Check loaded partition for duplicates
            let has_spilled_dup = 'has_spilled_dup: {
                let spill_state = self.spill_state.as_ref().expect("spill state exists");
                let Some(partition) = spill_state.find_partition(partition_idx) else {
                    break 'has_spilled_dup false;
                };
                if partition.buckets.is_empty() {
                    break 'has_spilled_dup false;
                }
                let bucket_idx = (hash as usize) % partition.buckets.len();
                let bucket = &partition.buckets[bucket_idx];
                bucket.entries.iter().any(|entry| {
                    entry.hash == hash
                        && keys_equal_distinct(&entry.key_values, key_refs, &self.collations)
                })
            };
            if has_spilled_dup {
                return Ok(IOResult::Done(false));
            }

            let entry_size = HashEntry::size_from_values(key_values, &[]);
            if let Some(c) = self.spill_partitions_for_entry(entry_size, metrics.as_deref_mut())? {
                if !c.succeeded() {
                    return Ok(IOResult::IO(IOCompletions(c)));
                }
            }

            {
                let spill_state = self.spill_state.as_mut().expect("spill state exists");
                let owned_keys: HashKeyValues = if let [only] = key_values {
                    HashKeyValues::from([only.clone()])
                } else {
                    HashKeyValues::from(key_values.iter().cloned().try_collect::<Vec<Value>>()?)
                };
                spill_state.partition_buffers[partition_idx].insert(HashEntry::new(
                    hash,
                    owned_keys,
                    0,
                ))?;
            }
            self.num_entries += 1;
            self.mem_used += entry_size;
            return Ok(IOResult::Done(true));
        }

        // Non-spilled mode: walk bucket chain in flat slab
        let bucket_idx = Self::bucket_of(hash, self.heads.len());
        let mut cur = self.heads.get(bucket_idx).copied().unwrap_or(EMPTY_ENTRY);
        while cur != EMPTY_ENTRY {
            let Some(i) = usize::try_from(cur).ok() else {
                break;
            };
            let Some(entry) = self.entries.get(i) else {
                break;
            };
            if entry.hash == hash
                && keys_equal_distinct(&entry.key_values, key_refs, &self.collations)
            {
                return Ok(IOResult::Done(false));
            }
            cur = self.next.get(i).copied().unwrap_or(EMPTY_ENTRY);
        }

        let entry_size = HashEntry::size_from_values(key_values, &[]);
        if self.over_budget(entry_size) {
            if self.spill_state.is_none() {
                let partition_count = self.choose_partition_count(entry_size);
                let partitioning = Partitioning::new(partition_count);
                self.spill_state = Some(SpillState::new(&self.io, self.temp_store, partitioning)?);
                self.redistribute_to_partitions()?;
                self.state = HashTableState::Spilled;
            }
            return self.insert_distinct(key_values, key_refs, metrics);
        }

        let bucket_idx = if self.grow_buckets(entry_size)? {
            Self::bucket_of(hash, self.heads.len())
        } else {
            bucket_idx
        };
        let mut owned_keys = HashKeyValues::new();
        if key_values.len() > owned_keys.inline_size() {
            owned_keys
                .try_reserve_exact(key_values.len())
                .map_err(|_| LimboError::OutOfMemory)?;
        }
        for v in key_values {
            owned_keys.push(v.try_clone()?);
        }
        self.push_slab_entry(bucket_idx, HashEntry::new(hash, owned_keys, 0))?;
        self.num_entries += 1;
        self.mem_used += entry_size;
        Ok(IOResult::Done(true))
    }

    /// AUDIT 3.1: the main table's bucket for a hash. The bucket count is a power of two
    /// (`new`, `grow_buckets`), so this is the hash's low bits; the spill partitions take its
    /// high bits (`Partitioning::index`), so the two stay independent.
    #[inline(always)]
    fn bucket_of(hash: u64, buckets: usize) -> usize {
        let mask = buckets.wrapping_sub(1);
        // The masked bits are at most `mask`, a usize, so they fit one.
        usize::try_from(hash & u64::try_from(mask).unwrap_or(u64::MAX)).unwrap_or(mask)
    }

    /// Whether an entry of `entry_size` bytes would take the table past its memory budget. A sum
    /// past usize::MAX is past every budget.
    fn over_budget(&self, entry_size: usize) -> bool {
        self.mem_used
            .checked_add(self.bucket_mem)
            .and_then(|used| used.checked_add(entry_size))
            .is_none_or(|used| used > self.mem_budget)
    }

    /// AUDIT 3.1: doubles the in-memory bucket head array once the table holds as many entries as
    /// it has buckets, so chains stay short however many rows arrive (Knuth TAOCP vol. 3, 6.4).
    /// Entries stay in place in `self.entries`; only `heads` and `next` links are rebuilt with
    /// zero per-bucket allocations and zero entry moves.
    fn grow_buckets(&mut self, entry_size: usize) -> Result<bool> {
        let len = self.heads.len();
        if self.num_entries < len || self.spill_state.is_some() || len == 0 {
            return Ok(false);
        }
        let Some(new_len) = len.checked_mul(2) else {
            return Ok(false);
        };
        let added = len.saturating_mul(std::mem::size_of::<u32>());
        if self
            .mem_used
            .saturating_add(self.bucket_mem)
            .saturating_add(added)
            .saturating_add(entry_size)
            > self.mem_budget
        {
            return Ok(false);
        }
        let mut heads: Vec<u32> = Vec::new();
        heads.try_reserve_exact(new_len)?;
        heads.resize(new_len, EMPTY_ENTRY);
        self.non_empty_buckets.clear();
        for (i, entry) in self.entries.iter().enumerate() {
            let entry_idx = u32::try_from(i).map_err(|_| LimboError::IntegerOverflow)?;
            let bucket_idx = Self::bucket_of(entry.hash, new_len);
            let Some(head) = heads.get_mut(bucket_idx) else {
                return Err(LimboError::InternalError("hash bucket out of range".into()));
            };
            let prev_head = *head;
            if prev_head == EMPTY_ENTRY {
                self.non_empty_buckets.try_push(bucket_idx)?;
            }
            if let Some(next_slot) = self.next.get_mut(i) {
                *next_slot = prev_head;
            }
            *head = entry_idx;
        }
        self.heads = heads;
        self.bucket_mem = self.bucket_mem.saturating_add(added);
        Ok(true)
    }

    /// Clear all entries and reset spill state.
    pub fn clear(&mut self) -> Result<()> {
        self.merge_order.clear();
        self.merge_run = 0;
        self.merge_next = 0;
        self.probe_current_entry = None;
        self.probe_next_entry = EMPTY_ENTRY;
        if self.num_entries == 0 && self.spill_state.is_none() {
            self.state = HashTableState::Building;
            self.current_probe_keys = None;
            self.current_probe_hash = None;
            self.probe_bucket_idx = 0;
            self.probe_entry_idx = 0;
            self.current_spill_partition_idx = 0;
            self.loaded_partitions_lru.borrow_mut().clear();
            self.loaded_partitions_mem = 0;
            self.non_empty_buckets.clear();
            self.probe_spill_state = None;
            self.grace_state = None;
            return Ok(());
        }

        if self.spill_state.is_some() {
            // Drop spilled partitions and reset bucket heads.
            self.spill_state = None;
            let bucket_count = self.initial_buckets.max(1);
            let mut heads = Vec::new();
            heads.try_reserve_exact(bucket_count)?;
            heads.resize(bucket_count, EMPTY_ENTRY);
            self.heads = heads;
            self.bucket_mem = 0;
            self.non_empty_buckets.clear();
        } else {
            for &idx in &self.non_empty_buckets {
                if let Some(head) = self.heads.get_mut(idx) {
                    *head = EMPTY_ENTRY;
                }
            }
            self.non_empty_buckets.clear();
        }
        self.entries.clear();
        self.next.clear();
        self.matched_bits.clear();

        self.num_entries = 0;
        self.mem_used = 0;
        self.state = HashTableState::Building;
        self.current_probe_keys = None;
        self.current_probe_hash = None;
        self.probe_bucket_idx = 0;
        self.probe_entry_idx = 0;
        self.current_spill_partition_idx = 0;
        self.loaded_partitions_lru.borrow_mut().clear();
        self.loaded_partitions_mem = 0;
        Ok(())
    }

    /// Redistribute existing slab entries into partition buffers for grace hash join.
    fn redistribute_to_partitions(&mut self) -> Result<()> {
        let partitioning = {
            let spill_state = self.spill_state.as_ref().expect("spill state must exist");
            spill_state.partitioning
        };
        for entry in self.entries.drain(..) {
            let partition_idx = partitioning.index(entry.hash);
            self.spill_state
                .as_mut()
                .expect("spill state must exist")
                .partition_buffers[partition_idx]
                .insert(entry)?;
        }
        // Clear in-memory slab metadata; spilled partitions will have their own.
        self.heads.clear();
        self.next.clear();
        self.matched_bits.clear();
        self.non_empty_buckets.clear();
        self.bucket_mem = 0;
        Ok(())
    }

    /// Return the next partition which should be spilled to disk, for simplicity,
    /// we always select the largest non-empty partition buffer.
    fn next_partition_to_spill(&self, _required_free: usize) -> Option<usize> {
        let spill_state = self.spill_state.as_ref()?;
        spill_state
            .partition_buffers
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.is_empty())
            .max_by_key(|(_, p)| p.mem_used)
            .map(|(idx, _)| idx)
    }

    /// Spill the given partition buffer to disk and return the pending completion.
    /// Uses single-pass serialization directly into the I/O buffer to avoid intermediate copies.
    fn spill_partition(
        &mut self,
        partition_idx: usize,
        metrics: Option<&mut HashJoinMetrics>,
    ) -> Result<Option<Completion>> {
        let mut metrics = metrics;
        let spill_state = self.spill_state.as_mut().expect("Spill state must exist");
        let partition = &spill_state.partition_buffers[partition_idx];
        if partition.is_empty() {
            return Ok(None);
        }

        // Phase 1: Calculate sizes and cache them to avoid recomputation
        let num_entries = partition.entries.len();
        let mut entry_sizes = Vec::try_with_capacity_ext(num_entries)?;
        let mut total_size = 0usize;
        for entry in &partition.entries {
            let entry_size = entry.serialized_size();
            entry_sizes
                .push_within_capacity(entry_size)
                .expect("entry sizes vector was preallocated");
            total_size += varint_len(entry_size as u64) + entry_size;
        }

        // Allocate I/O buffer and serialize using cached sizes
        let buffer = Buffer::new_temporary(total_size);
        let buf = buffer.as_mut_slice();
        let mut offset = 0;

        for (entry, &entry_size) in partition.entries.iter().zip(entry_sizes.iter()) {
            offset += write_varint(&mut buf[offset..], entry_size as u64);
            offset += entry.serialize_to_slice(&mut buf[offset..]);
        }

        turso_assert!(offset == total_size, "serialized size mismatch");

        let file_offset = spill_state.next_spill_offset;
        let data_size = total_size;
        let num_entries = spill_state.partition_buffers[partition_idx].entries.len();
        let mem_freed = spill_state.partition_buffers[partition_idx].mem_used;

        spill_state.partition_buffers[partition_idx].clear();

        // Find existing partition or create new one
        let io_state = if let Some(existing) = spill_state.find_partition_mut(partition_idx) {
            existing.add_chunk(file_offset, data_size, num_entries)?;
            if let Some(metrics) = metrics.as_deref_mut() {
                metrics.spill_bytes_written =
                    metrics.spill_bytes_written.saturating_add(data_size as u64);
                metrics.spill_chunks = metrics.spill_chunks.saturating_add(1);
                metrics.spill_max_chunks_per_partition = metrics
                    .spill_max_chunks_per_partition
                    .max(existing.chunks.len() as u64);
                metrics.spill_max_partition_bytes = metrics
                    .spill_max_partition_bytes
                    .max(existing.total_size_bytes() as u64);
            }
            existing.io_state.clone()
        } else {
            let mut new_partition = SpilledPartition::new(partition_idx);
            new_partition.add_chunk(file_offset, data_size, num_entries)?;
            if let Some(metrics) = metrics {
                metrics.spill_bytes_written =
                    metrics.spill_bytes_written.saturating_add(data_size as u64);
                metrics.spill_chunks = metrics.spill_chunks.saturating_add(1);
                metrics.spill_max_chunks_per_partition = metrics
                    .spill_max_chunks_per_partition
                    .max(new_partition.chunks.len() as u64);
                metrics.spill_max_partition_bytes = metrics
                    .spill_max_partition_bytes
                    .max(new_partition.total_size_bytes() as u64);
            }
            let io_state = new_partition.io_state.clone();
            spill_state.partitions.try_push(new_partition)?;
            io_state
        };

        io_state.set(SpillIOState::WaitingForWrite);

        let buffer_ref = Arc::new(buffer);
        let write_complete = Box::new(move |res: Result<i32, crate::CompletionError>| match res {
            Ok(_) => {
                tracing::trace!("Successfully wrote spilled partition to disk");
                io_state.set(SpillIOState::WriteComplete);
            }
            Err(e) => {
                tracing::error!("Error writing spilled partition to disk: {e:?}");
                io_state.set(SpillIOState::Error);
            }
        });

        let completion = Completion::new_write(write_complete);
        let file = spill_state.temp_file.file.clone();
        let completion = file.pwrite(file_offset, buffer_ref, completion)?;

        // Update state
        self.mem_used -= mem_freed;
        spill_state.next_spill_offset += data_size as u64;
        Ok(Some(completion))
    }

    /// Spill multiple partitions in a single I/O operation.
    /// This batches the work to reduce syscall overhead when freeing large amounts of memory.
    fn spill_multiple_partitions(
        &mut self,
        partition_indices: &[usize],
        metrics: Option<&mut HashJoinMetrics>,
    ) -> Result<Option<Completion>> {
        let mut metrics = metrics;
        if partition_indices.is_empty() {
            return Ok(None);
        }

        // If only one partition, use the simpler single-partition path
        if partition_indices.len() == 1 {
            return self.spill_partition(partition_indices[0], metrics);
        }

        let spill_state = self.spill_state.as_mut().expect("Spill state must exist");

        // Phase 1: Calculate total size and per-partition metadata
        struct PartitionMeta {
            idx: usize,
            num_entries: usize,
            data_size: usize,
            mem_freed: usize,
            entry_sizes: Vec<usize>,
        }

        let mut metas = Vec::try_with_capacity_ext(partition_indices.len())?;
        let mut total_size = 0usize;

        for &partition_idx in partition_indices {
            let partition = &spill_state.partition_buffers[partition_idx];
            if partition.is_empty() {
                continue;
            }

            let mut entry_sizes = Vec::try_with_capacity_ext(partition.entries.len())?;
            let mut partition_size = 0usize;
            for entry in &partition.entries {
                let entry_size = entry.serialized_size();
                entry_sizes
                    .push_within_capacity(entry_size)
                    .expect("entry sizes vector was preallocated");
                partition_size += varint_len(entry_size as u64) + entry_size;
            }

            metas.try_push(PartitionMeta {
                idx: partition_idx,
                num_entries: partition.entries.len(),
                data_size: partition_size,
                mem_freed: partition.mem_used,
                entry_sizes,
            })?;
            total_size += partition_size;
        }

        if metas.is_empty() {
            return Ok(None);
        }

        // Allocate single I/O buffer and serialize all partitions
        let buffer = Buffer::new_temporary(total_size);
        let buf = buffer.as_mut_slice();
        let mut offset = 0;
        let base_file_offset = spill_state.next_spill_offset;

        // Track where each partition's data starts in the buffer
        let mut partition_offsets = Vec::try_with_capacity_ext(metas.len())?;

        for meta in &metas {
            partition_offsets
                .push_within_capacity(offset)
                .expect("partition offsets vector was preallocated");
            let partition = &spill_state.partition_buffers[meta.idx];

            for (entry, &entry_size) in partition.entries.iter().zip(meta.entry_sizes.iter()) {
                offset += write_varint(&mut buf[offset..], entry_size as u64);
                offset += entry.serialize_to_slice(&mut buf[offset..]);
            }
        }

        turso_assert!(offset == total_size, "serialized size mismatch");

        // Update partition metadata and clear buffers
        let mut total_mem_freed = 0usize;
        let mut io_states = Vec::try_with_capacity_ext(metas.len())?;

        for (meta, &partition_offset) in metas.iter().zip(partition_offsets.iter()) {
            let file_offset = base_file_offset + partition_offset as u64;

            spill_state.partition_buffers[meta.idx].clear();
            total_mem_freed += meta.mem_freed;

            // Find existing partition or create new one
            let io_state = if let Some(existing) = spill_state.find_partition_mut(meta.idx) {
                existing.add_chunk(file_offset, meta.data_size, meta.num_entries)?;
                if let Some(metrics) = metrics.as_deref_mut() {
                    metrics.spill_bytes_written = metrics
                        .spill_bytes_written
                        .saturating_add(meta.data_size as u64);
                    metrics.spill_chunks = metrics.spill_chunks.saturating_add(1);
                    metrics.spill_max_chunks_per_partition = metrics
                        .spill_max_chunks_per_partition
                        .max(existing.chunks.len() as u64);
                    metrics.spill_max_partition_bytes = metrics
                        .spill_max_partition_bytes
                        .max(existing.total_size_bytes() as u64);
                }
                existing.io_state.clone()
            } else {
                let mut new_partition = SpilledPartition::new(meta.idx);
                new_partition.add_chunk(file_offset, meta.data_size, meta.num_entries)?;
                if let Some(metrics) = metrics.as_deref_mut() {
                    metrics.spill_bytes_written = metrics
                        .spill_bytes_written
                        .saturating_add(meta.data_size as u64);
                    metrics.spill_chunks = metrics.spill_chunks.saturating_add(1);
                    metrics.spill_max_chunks_per_partition = metrics
                        .spill_max_chunks_per_partition
                        .max(new_partition.chunks.len() as u64);
                    metrics.spill_max_partition_bytes = metrics
                        .spill_max_partition_bytes
                        .max(new_partition.total_size_bytes() as u64);
                }
                let io_state = new_partition.io_state.clone();
                spill_state.partitions.try_push(new_partition)?;
                io_state
            };

            io_state.set(SpillIOState::WaitingForWrite);
            io_states.try_push(io_state)?;
        }

        // Submit single I/O write
        let buffer_ref = Arc::new(buffer);
        let _buffer_ref_clone = buffer_ref.clone();
        let write_complete = Box::new(move |res: Result<i32, crate::CompletionError>| match res {
            Ok(_) => {
                let _buf = _buffer_ref_clone.clone();
                tracing::trace!(
                    "Successfully wrote {} batched partitions to disk",
                    io_states.len()
                );
                for io_state in &io_states {
                    io_state.set(SpillIOState::WriteComplete);
                }
            }
            Err(e) => {
                tracing::error!("Error writing batched partitions to disk: {e:?}");
                for io_state in &io_states {
                    io_state.set(SpillIOState::Error);
                }
            }
        });

        let completion = Completion::new_write(write_complete);
        let file = spill_state.temp_file.file.clone();
        let completion = file.pwrite(base_file_offset, buffer_ref, completion)?;

        // Update state
        self.mem_used -= total_mem_freed;
        spill_state.next_spill_offset += total_size as u64;
        Ok(Some(completion))
    }

    /// Spill as many whole partitions as needed to keep the incoming entry within budget.
    /// Uses batch spilling to combine multiple partitions into a single I/O operation.
    fn spill_partitions_for_entry(
        &mut self,
        entry_size: usize,
        metrics: Option<&mut HashJoinMetrics>,
    ) -> Result<Option<Completion>> {
        if self.mem_used + entry_size <= self.mem_budget {
            return Ok(None);
        }

        // Collect all partitions that need to be spilled
        let mut partitions_to_spill = vec![];
        let mut projected_mem_used = self.mem_used;

        let spill_state = self.spill_state.as_ref().expect("spill state must exist");

        // Sort partitions by size (largest first) to minimize number of spills
        let mut candidates: Vec<(usize, usize)> = spill_state
            .partition_buffers
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.is_empty())
            .map(|(idx, p)| (idx, p.mem_used))
            .try_collect()?;
        candidates.sort_by_key(|c| std::cmp::Reverse(c.1)); // Sort descending by mem_used

        for (partition_idx, mem_used) in candidates {
            if projected_mem_used + entry_size <= self.mem_budget {
                break;
            }
            partitions_to_spill.try_push(partition_idx)?;
            projected_mem_used -= mem_used;
        }

        if partitions_to_spill.is_empty() {
            return Ok(None);
        }

        self.spill_multiple_partitions(&partitions_to_spill, metrics)
    }

    /// Convert a never-spilled partition buffer into in-memory buckets for probing.
    fn materialize_partition_in_memory(&mut self, partition_idx: usize) -> Result<()> {
        let spill_state = self.spill_state.as_mut().expect("spill state must exist");
        if spill_state.find_partition(partition_idx).is_some() {
            return Ok(());
        }

        let partition_buffer = &mut spill_state.partition_buffers[partition_idx];
        if partition_buffer.is_empty() {
            return Ok(());
        }

        let entries = std::mem::replace(&mut partition_buffer.entries, vec![]);
        // we don't change self.mem_used here, as these entries
        // were always in memory. we’re just changing their layout
        partition_buffer.mem_used = 0;

        let bucket_count = entries.len().next_power_of_two().max(64);
        let mut buckets: Vec<_> = (0..bucket_count).map(|_| HashBucket::new()).try_collect()?;
        for entry in entries {
            let bucket_idx = (entry.hash as usize) % bucket_count;
            buckets[bucket_idx].insert(entry)?;
        }

        let matched_bits = if self.track_matched {
            buckets
                .iter()
                .map(|b| vec![false; b.entries.len()])
                .try_collect()?
        } else {
            vec![]
        };
        let mut partition = SpilledPartition::new(partition_idx);
        partition.state = PartitionState::InMemory;
        partition.buckets = buckets;
        partition.matched_bits = matched_bits;
        partition.resident_mem = 0;
        spill_state.partitions.try_push(partition)?;
        Ok(())
    }

    /// Finalize the build phase and prepare for probing.
    /// If spilled, flushes remaining in-memory partition entries to disk.
    pub fn finalize_build(&mut self, metrics: Option<&mut HashJoinMetrics>) -> IOResultOr<()> {
        let mut metrics = metrics;
        turso_assert!(
            self.state == HashTableState::Building || self.state == HashTableState::Spilled,
            "Cannot finalize build in unexpected state",
            { "state": format!("{:?}", self.state) }
        );

        if self.spill_state.is_some() {
            {
                // Check for pending writes from previous call
                let spill_state = self.spill_state.as_ref().expect("spill state must exist");
                for spilled in &spill_state.partitions {
                    if matches!(spilled.io_state.get(), SpillIOState::WaitingForWrite) {
                        io_yield_one!(Completion::new_yield());
                    }
                }
            }
            // Determine which partitions need to spill vs stay in memory without holding
            // a mutable borrow across the spill/materialize calls.
            let mut spill_targets = vec![];
            let mut materialize_targets = vec![];
            {
                let spill_state = self.spill_state.as_ref().expect("spill state must exist");
                for partition_idx in 0..spill_state.partitioning.count {
                    let partition = &spill_state.partition_buffers[partition_idx];
                    if partition.is_empty() {
                        continue;
                    }
                    if spill_state.find_partition(partition_idx).is_some() {
                        spill_targets.try_push(partition_idx)?;
                    } else {
                        materialize_targets.try_push(partition_idx)?;
                    }
                }
            }
            for partition_idx in spill_targets {
                if let Some(completion) =
                    self.spill_partition(partition_idx, metrics.as_deref_mut())?
                {
                    // Return I/O completion to caller, they will re-enter after completion
                    if !completion.finished() {
                        io_yield_one!(completion);
                    }
                }
            }
            for partition_idx in materialize_targets {
                self.materialize_partition_in_memory(partition_idx)?;
            }
        } else if self.entries.len() > self.non_empty_buckets.len() {
            // Restore FIFO insertion order for duplicate keys / colliding buckets in one
            // cache-sequential reverse pass over `entries` (skipped in O(1) when every non-empty
            // bucket holds a single entry).
            for &b in &self.non_empty_buckets {
                if let Some(h) = self.heads.get_mut(b) {
                    *h = EMPTY_ENTRY;
                }
            }
            let num_buckets = self.heads.len();
            for (idx, entry) in self.entries.iter().enumerate().rev() {
                let bucket_idx = Self::bucket_of(entry.hash, num_buckets);
                if let (Ok(entry_u32), Some(head), Some(next_slot)) = (
                    u32::try_from(idx),
                    self.heads.get_mut(bucket_idx),
                    self.next.get_mut(idx),
                ) {
                    *next_slot = *head;
                    *head = entry_u32;
                }
            }
        }
        self.current_spill_partition_idx = 0;
        // Amphora (QT.5 step 4, lane qs5-21): a merge's rows sorted by key, in memory, or probed
        // by hash when they spilled or their order would pass the budget.
        self.merge_order.clear();
        self.merge_run = 0;
        self.merge_next = 0;
        if self.merge
            && self.num_entries > 0
            && (self.spill_state.is_some() || !self.build_merge_order()?)
        {
            if let Some(metrics) = metrics {
                metrics.merge_tables_hashed = metrics.merge_tables_hashed.saturating_add(1);
            }
        }
        self.state = HashTableState::Probing;
        Ok(IOResult::Done(()))
    }

    /// Probe the hash table with the given keys, returns the first matching entry if found.
    /// NOTE: Calling `probe` on a spilled table requires the relevant partition to be loaded.
    /// Returns None immediately if any probe key is NULL since NULL != NULL in SQL.
    pub fn probe(
        &mut self,
        probe_keys: Vec<Value>,
        metrics: Option<&mut HashJoinMetrics>,
    ) -> Result<Option<&HashEntry>> {
        turso_assert!(
            self.state == HashTableState::Probing,
            "Cannot probe hash table in unexpected state",
            { "state": format!("{:?}", self.state) }
        );

        // Skip probing if any key is NULL - NULL can never match anything in SQL
        if has_null_key(&probe_keys) {
            self.current_probe_keys = Some(probe_keys);
            self.current_probe_hash = None;
            return Ok(None);
        }

        // Amphora (QT.5 step 4, lane qs5-21): a sort-merge join's table, by merging.
        if !self.merge_order.is_empty() {
            self.current_probe_keys = Some(probe_keys);
            self.current_probe_hash = None;
            let mut metrics = metrics;
            self.record_probe_call(metrics.as_deref_mut());
            return self.merge_probe(metrics);
        }

        let hash = hash_join_values(&probe_keys, &self.collations);
        self.current_probe_keys = Some(probe_keys);
        self.current_probe_hash = Some(hash);

        // Reset probe state
        self.probe_entry_idx = 0;

        if self.spill_state.is_some() {
            // In spilled mode, search through loaded entries from spilled partitions
            // that match this probe key's partition
            let partitioning = {
                let spill_state = self.spill_state.as_ref().expect("spill state must exist");
                spill_state.partitioning
            };
            let target_partition = partitioning.index(hash);
            self.record_probe_call(metrics);
            self.touch_partition_lru(target_partition);

            let bucket_idx = {
                let spill_state = self.spill_state.as_ref().expect("spill state must exist");
                let Some(partition) = spill_state.find_partition(target_partition) else {
                    return Ok(None);
                };
                if partition.buckets.is_empty() {
                    return Ok(None);
                }
                (hash as usize) % partition.buckets.len()
            };

            self.probe_bucket_idx = bucket_idx;
            self.current_spill_partition_idx = target_partition;

            let match_idx = {
                let key = self.current_probe_keys.as_deref().unwrap_or_default();
                let spill_state = self.spill_state.as_ref().expect("spill state must exist");
                let Some(partition) = spill_state.find_partition(target_partition) else {
                    return Ok(None);
                };
                let bucket = &partition.buckets[bucket_idx];
                let mut found = None;
                for (idx, entry) in bucket.entries.iter().enumerate() {
                    if entry.hash == hash
                        && keys_equal_values(&entry.key_values, key, &self.collations)
                    {
                        found = Some(idx);
                        break;
                    }
                }
                found
            };

            if let Some(idx) = match_idx {
                self.probe_entry_idx = idx + 1;
                let spill_state = self.spill_state.as_ref().expect("spill state must exist");
                let Some(partition) = spill_state.find_partition(target_partition) else {
                    return Ok(None);
                };
                let bucket = &partition.buckets[bucket_idx];
                return Ok(bucket.entries.get(idx));
            }
            Ok(None)
        } else {
            // Normal mode - walk flat slab bucket chain
            self.record_probe_call(metrics);
            let bucket_idx = Self::bucket_of(hash, self.heads.len());
            self.probe_current_entry = None;
            self.probe_next_entry = EMPTY_ENTRY;
            let mut cur = self.heads.get(bucket_idx).copied().unwrap_or(EMPTY_ENTRY);
            let key = self.current_probe_keys.as_deref().unwrap_or_default();
            let mut match_idx: Option<(u32, usize, u32)> = None;
            while cur != EMPTY_ENTRY {
                let Some(i) = usize::try_from(cur).ok() else {
                    break;
                };
                let Some(entry) = self.entries.get(i) else {
                    break;
                };
                let next_idx = self.next.get(i).copied().unwrap_or(EMPTY_ENTRY);
                if entry.hash == hash
                    && keys_equal_values(&entry.key_values, key, &self.collations)
                {
                    match_idx = Some((cur, i, next_idx));
                    break;
                }
                cur = next_idx;
            }

            if let Some((cur_u32, idx, next_idx)) = match_idx {
                self.probe_current_entry = Some(cur_u32);
                self.probe_next_entry = next_idx;
                return Ok(self.entries.get(idx));
            }
            Ok(None)
        }
    }

    /// AUDIT 3.1: `keys` copied into the last probe's key buffer, for [Self::probe]: a probe row
    /// allocates nothing once the buffer has held keys its size.
    pub fn probe_keys_from<'a>(
        &mut self,
        keys: impl ExactSizeIterator<Item = &'a Value>,
    ) -> Result<Vec<Value>> {
        self.current_probe_hash = None;
        let mut buf = self.current_probe_keys.take().unwrap_or_default();
        refill_keys(&mut buf, keys)?;
        Ok(buf)
    }

    /// Get the next matching entry for the current probe keys.
    pub fn next_match(&mut self) -> Result<Option<&HashEntry>> {
        turso_assert!(
            self.state == HashTableState::Probing || self.state == HashTableState::GraceProcessing,
            "Cannot get next match in unexpected state",
            { "state": format!("{:?}", self.state) }
        );

        turso_assert!(self.current_probe_keys.is_some(), "probe keys must be set");
        if !self.merge_order.is_empty() {
            return self.merge_next_match();
        }
        let Some(probe_keys) = self.current_probe_keys.as_deref() else {
            return Ok(None);
        };
        let hash = match self.current_probe_hash {
            Some(h) => h,
            None => {
                let h = hash_join_values(probe_keys, &self.collations);
                self.current_probe_hash = Some(h);
                h
            }
        };

        if let Some(spill_state) = self.spill_state.as_ref() {
            let partition_idx = self.current_spill_partition_idx;

            turso_assert_eq!(partition_idx, self.partition_index(hash));
            let Some(partition) = spill_state.find_partition(partition_idx) else {
                return Ok(None);
            };
            if partition.buckets.is_empty() {
                return Ok(None);
            }

            let bucket = &partition.buckets[self.probe_bucket_idx];
            // Continue from where we left off
            for idx in self.probe_entry_idx..bucket.entries.len() {
                let entry = &bucket.entries[idx];
                if entry.hash == hash
                    && keys_equal_values(&entry.key_values, probe_keys, &self.collations)
                {
                    self.probe_entry_idx = idx + 1;
                    return Ok(Some(entry));
                }
            }
            Ok(None)
        } else {
            // Non-spilled case: continue walking flat slab bucket chain
            let mut cur = self.probe_next_entry;
            while cur != EMPTY_ENTRY {
                let Some(i) = usize::try_from(cur).ok() else {
                    break;
                };
                let Some(entry) = self.entries.get(i) else {
                    break;
                };
                let next_idx = self.next.get(i).copied().unwrap_or(EMPTY_ENTRY);
                if entry.hash == hash
                    && keys_equal_values(&entry.key_values, probe_keys, &self.collations)
                {
                    self.probe_current_entry = Some(cur);
                    self.probe_next_entry = next_idx;
                    return Ok(self.entries.get(i));
                }
                cur = next_idx;
            }
            self.probe_next_entry = EMPTY_ENTRY;
            Ok(None)
        }
    }

    /// Mark the current matched entry as "matched" for outer join tracking.
    /// Must be called after a successful probe/next_match.
    pub fn mark_current_matched(&mut self) {
        if !self.track_matched {
            return;
        }
        if let Some(spill_state) = &mut self.spill_state {
            let entry_idx = self
                .probe_entry_idx
                .checked_sub(1)
                .expect("mark_current_matched called without prior probe match");
            let bucket_idx = self.probe_bucket_idx;
            let partition_idx = self.current_spill_partition_idx;
            let partition = spill_state
                .find_partition_mut(partition_idx)
                .expect("spilled partition missing during mark_current_matched");
            partition.matched_bits[bucket_idx][entry_idx] = true;
        } else if let Some(entry_u32) = self.probe_current_entry {
            if let Ok(idx) = usize::try_from(entry_u32) {
                if let Some(bit) = self.matched_bits.get_mut(idx) {
                    *bit = true;
                }
            }
        }
    }

    /// Mark the current matched entry, returning whether it was unmarked before: a semi join emits
    /// its build row only then (G6 step 5). Must be called after a successful probe/next_match on
    /// a table that tracks matches; anything else is an internal error, not a panic.
    pub fn mark_current_matched_first(&mut self) -> Result<bool> {
        if !self.track_matched {
            return Err(LimboError::InternalError(
                "semi join's hash table does not track matches".to_string(),
            ));
        }
        if let Some(spill_state) = &mut self.spill_state {
            let entry_idx = self.probe_entry_idx.checked_sub(1).ok_or_else(|| {
                LimboError::InternalError("hash first-match test without a probe match".to_string())
            })?;
            let bucket_idx = self.probe_bucket_idx;
            let partition_idx = self.current_spill_partition_idx;
            let partition = spill_state
                .find_partition_mut(partition_idx)
                .ok_or_else(|| {
                    LimboError::InternalError(format!(
                        "spilled partition {partition_idx} missing at a hash first-match test"
                    ))
                })?;
            let bit = partition
                .matched_bits
                .get_mut(bucket_idx)
                .and_then(|bucket| bucket.get_mut(entry_idx))
                .ok_or_else(|| {
                    LimboError::InternalError(format!(
                        "hash first-match test past its bucket ({bucket_idx}, {entry_idx})"
                    ))
                })?;
            let first = !*bit;
            *bit = true;
            return Ok(first);
        }
        let entry_u32 = self.probe_current_entry.ok_or_else(|| {
            LimboError::InternalError("hash first-match test without a probe match".to_string())
        })?;
        let idx = usize::try_from(entry_u32).map_err(|_| LimboError::IntegerOverflow)?;
        let bit = self.matched_bits.get_mut(idx).ok_or_else(|| {
            LimboError::InternalError(format!("hash first-match test past slab ({idx})"))
        })?;
        let first = !*bit;
        *bit = true;
        Ok(first)
    }

    /// Reset all matched_bits to false. Called at the start of each outer-loop
    /// iteration so that marks from a previous iteration don't suppress NULL-fill
    /// rows in the current one.
    pub fn reset_matched_bits(&mut self) {
        if let Some(spill_state) = self.spill_state.as_mut() {
            for partition in &mut spill_state.partitions {
                for bits in &mut partition.matched_bits {
                    bits.fill(false);
                }
            }
        }
        self.matched_bits.fill(false);
    }

    /// Reset the unmatched scan state to the beginning.
    pub fn begin_unmatched_scan(&mut self) {
        self.unmatched_scan_bucket = 0;
        self.unmatched_scan_entry = 0;
        self.unmatched_scan_partition = 0;
    }

    /// Advance to the next unmatched entry in the hash table.
    /// Returns the entry if found, or None when the scan is complete
    /// (or a spilled partition needs loading).
    pub fn next_unmatched(&mut self) -> Option<&HashEntry> {
        let has_spill_state = self.spill_state.is_some();
        let in_grace = self.grace_state.is_some();
        let has_pending_grace = !in_grace && self.has_grace_partitions();

        if has_spill_state {
            if in_grace {
                // During grace: scan only the partition currently owned by grace.
                return self.next_unmatched_current_grace_partition();
            }
            if has_pending_grace {
                // Before grace starts, emit unmatched rows from never-spilled in-memory
                // partitions here. Grace will handle only partitions that actually have
                // disk chunks, so skipping these would lose unmatched build rows.
                return self.next_unmatched_spilled_in_memory_only();
            }
            return self.next_unmatched_spilled();
        }
        self.next_unmatched_main_buckets()
    }

    fn next_unmatched_main_buckets(&mut self) -> Option<&HashEntry> {
        while self.unmatched_scan_entry < self.entries.len() {
            let idx = self.unmatched_scan_entry;
            self.unmatched_scan_entry = self.unmatched_scan_entry.saturating_add(1);
            if !self.matched_bits.get(idx).copied().unwrap_or(false) {
                return self.entries.get(idx);
            }
        }
        None
    }

    /// Advance to the next unmatched entry across spilled partitions.
    /// Returns None when a partition needs loading (caller must load and retry).
    fn next_unmatched_spilled(&mut self) -> Option<&HashEntry> {
        let spill_state = self.spill_state.as_ref()?;
        while self.unmatched_scan_partition < spill_state.partitions.len() {
            let partition = &spill_state.partitions[self.unmatched_scan_partition];
            if !partition.is_loaded() {
                // Partition not loaded yet — caller needs to load it.
                // Return None to signal we need I/O; caller will re-enter.
                return None;
            }
            while self.unmatched_scan_bucket < partition.buckets.len() {
                let bucket = &partition.buckets[self.unmatched_scan_bucket];
                let matched = &partition.matched_bits[self.unmatched_scan_bucket];
                while self.unmatched_scan_entry < bucket.entries.len() {
                    let idx = self.unmatched_scan_entry;
                    self.unmatched_scan_entry += 1;
                    if !matched[idx] {
                        return Some(&bucket.entries[idx]);
                    }
                }
                self.unmatched_scan_bucket += 1;
                self.unmatched_scan_entry = 0;
            }
            self.unmatched_scan_partition += 1;
            self.unmatched_scan_bucket = 0;
            self.unmatched_scan_entry = 0;
        }
        None
    }

    /// Scan unmatched rows from partitions that stayed resident in memory after spilling.
    /// These partitions have no disk chunks, so grace never revisits them.
    fn next_unmatched_spilled_in_memory_only(&mut self) -> Option<&HashEntry> {
        let spill_state = self.spill_state.as_ref()?;
        while self.unmatched_scan_partition < spill_state.partitions.len() {
            let partition = &spill_state.partitions[self.unmatched_scan_partition];
            if !partition.chunks.is_empty() {
                self.unmatched_scan_partition += 1;
                self.unmatched_scan_bucket = 0;
                self.unmatched_scan_entry = 0;
                continue;
            }

            turso_assert!(
                partition.is_loaded(),
                "in-memory partition unexpectedly unavailable during unmatched scan",
                { "partition_idx": partition.partition_idx }
            );

            while self.unmatched_scan_bucket < partition.buckets.len() {
                let bucket = &partition.buckets[self.unmatched_scan_bucket];
                let matched = &partition.matched_bits[self.unmatched_scan_bucket];
                while self.unmatched_scan_entry < bucket.entries.len() {
                    let idx = self.unmatched_scan_entry;
                    self.unmatched_scan_entry += 1;
                    if !matched[idx] {
                        return Some(&bucket.entries[idx]);
                    }
                }
                self.unmatched_scan_bucket += 1;
                self.unmatched_scan_entry = 0;
            }

            self.unmatched_scan_partition += 1;
            self.unmatched_scan_bucket = 0;
            self.unmatched_scan_entry = 0;
        }
        None
    }

    /// Scan unmatched entries only for the active grace partition.
    /// Grace unmatched emission happens partition-by-partition before eviction, so
    /// scanning any other loaded partition can duplicate or suppress rows.
    fn next_unmatched_current_grace_partition(&mut self) -> Option<&HashEntry> {
        let partition_idx = self.grace_state.as_ref()?.current_partition_idx()?;
        let spill_state = self.spill_state.as_ref()?;
        let partition = spill_state
            .find_partition(partition_idx)
            .expect("current grace partition missing from spill state");

        if !partition.is_loaded() {
            return None;
        }

        while self.unmatched_scan_bucket < partition.buckets.len() {
            let bucket = &partition.buckets[self.unmatched_scan_bucket];
            let matched = &partition.matched_bits[self.unmatched_scan_bucket];
            while self.unmatched_scan_entry < bucket.entries.len() {
                let idx = self.unmatched_scan_entry;
                self.unmatched_scan_entry += 1;
                if !matched[idx] {
                    return Some(&bucket.entries[idx]);
                }
            }
            self.unmatched_scan_bucket += 1;
            self.unmatched_scan_entry = 0;
        }
        None
    }

    /// Get the current partition index for unmatched scan (for spilled partition loading).
    pub fn unmatched_scan_current_partition(&self) -> Option<usize> {
        if let Some(grace_state) = self.grace_state.as_ref() {
            return grace_state.current_partition_idx();
        }
        if self.has_grace_partitions() {
            return None;
        }
        let spill_state = self.spill_state.as_ref()?;
        if self.unmatched_scan_partition < spill_state.partitions.len() {
            Some(spill_state.partitions[self.unmatched_scan_partition].partition_idx)
        } else {
            None
        }
    }

    /// Get the number of spilled partitions.
    pub fn num_partitions(&self) -> usize {
        self.spill_state
            .as_ref()
            .map(|s| s.partitions.len())
            .unwrap_or(0)
    }

    /// Check if a specific partition is loaded and ready for probing.
    pub fn is_partition_loaded(&self, partition_idx: usize) -> bool {
        self.spill_state
            .as_ref()
            .and_then(|s| s.find_partition(partition_idx))
            .is_some_and(|p| p.is_loaded())
    }

    /// Re-entrantly load spilled partitions from disk
    pub fn load_spilled_partition(
        &mut self,
        partition_idx: usize,
        mut metrics: Option<&mut HashJoinMetrics>,
    ) -> IOResultOr<()> {
        loop {
            // to avoid holding mut borrows, split this into two phases.
            let action = {
                let spill_state = match &mut self.spill_state {
                    Some(s) => s,
                    None => return Ok(IOResult::Done(())),
                };

                let spilled = match spill_state.find_partition_mut(partition_idx) {
                    Some(p) => p,
                    None => return Ok(IOResult::Done(())),
                };
                let io_state = spilled.io_state.get();

                if unlikely(matches!(io_state, SpillIOState::Error)) {
                    return Err(
                        LimboError::InternalError("hash join spill I/O failure".into()).into(),
                    );
                }
                // Already fully loaded
                if spilled.is_loaded() {
                    SpillAction::AlreadyLoaded
                } else if matches!(io_state, SpillIOState::WaitingForRead) {
                    // We've scheduled a read, caller must wait for completion.
                    SpillAction::WaitingForIO
                } else if matches!(io_state, SpillIOState::ReadComplete) {
                    SpillAction::ParseChunk { partition_idx }
                } else {
                    match spilled.current_chunk() {
                        Some(chunk) => {
                            let read_size = chunk.size_bytes;
                            let file_offset = chunk.file_offset;
                            let is_first_load = matches!(spilled.state, PartitionState::OnDisk)
                                && spilled.current_chunk_idx == 0;
                            if is_first_load {
                                let total_entries = spilled.total_num_entries();
                                let bucket_count = total_entries.next_power_of_two().max(64);
                                spilled.buckets =
                                    (0..bucket_count).map(|_| HashBucket::new()).try_collect()?;
                                spilled.parsed_entries = 0;
                                spilled.partial_entry.clear();
                            }

                            if read_size == 0 {
                                // Empty chunk: skip it and move to the next.
                                spilled.current_chunk_idx += 1;
                                if spilled.has_more_chunks() {
                                    SpillAction::Restart
                                } else {
                                    // No more chunks, but nothing to read; mark loaded.
                                    spilled.state = PartitionState::Loaded;
                                    SpillAction::NoChunks
                                }
                            } else {
                                // Non-empty chunk, schedule a read for it.
                                let buffer_len = spilled.buffer_len.clone();
                                let read_buffer_ref = spilled.read_buffer.clone();

                                spilled.io_state.set(SpillIOState::WaitingForRead);
                                spilled.state = PartitionState::Loading;

                                SpillAction::LoadChunk {
                                    read_size,
                                    file_offset,
                                    io_state: spilled.io_state.clone(),
                                    buffer_len,
                                    read_buffer_ref,
                                }
                            }
                        }
                        None => {
                            // No chunks at all: partition is logically empty, mark as loaded.
                            spilled.state = PartitionState::Loaded;
                            SpillAction::NoChunks
                        }
                    }
                }
            };

            match action {
                SpillAction::AlreadyLoaded => {
                    self.touch_partition_lru(partition_idx);
                    return Ok(IOResult::Done(()));
                }
                SpillAction::NoChunks => {
                    self.evict_partitions_to_fit(0, partition_idx);
                    self.record_partition_resident(partition_idx, 0);
                    return Ok(IOResult::Done(()));
                }
                SpillAction::NotFound => {
                    return Ok(IOResult::Done(()));
                }
                SpillAction::ParseChunk { partition_idx } => {
                    match self.parse_partition_chunk(partition_idx, metrics.as_deref_mut())? {
                        ParseChunkResult::MoreChunks => continue,
                        ParseChunkResult::Done { resident_mem } => {
                            self.evict_partitions_to_fit(resident_mem, partition_idx);
                            self.record_partition_resident(partition_idx, resident_mem);
                            return Ok(IOResult::Done(()));
                        }
                    }
                }
                SpillAction::WaitingForIO => {
                    io_yield_one!(Completion::new_yield());
                }
                SpillAction::Restart => {
                    // We advanced state (e.g., moved past an empty chunk or completed a chunk),
                    // so just loop again and recompute the next action.
                    continue;
                }
                SpillAction::LoadChunk {
                    read_size,
                    file_offset,
                    io_state,
                    buffer_len,
                    read_buffer_ref,
                } => {
                    let read_buffer = Arc::new(Buffer::new_temporary(read_size));
                    let read_complete = Box::new(
                        move |res: Result<(Arc<Buffer>, i32), CompletionError>| match res {
                            Ok((buf, bytes_read)) => {
                                tracing::trace!(
                                    "Completed read of spilled partition chunk: bytes_read={}",
                                    bytes_read
                                );
                                let mut persistent_buf = read_buffer_ref.write();
                                persistent_buf.clear();
                                persistent_buf
                                    .extend_from_slice(&buf.as_slice()[..bytes_read as usize]);
                                buffer_len.store(bytes_read as usize, atomic::Ordering::Release);
                                io_state.set(SpillIOState::ReadComplete);
                                None
                            }
                            Err(e) => {
                                tracing::error!("Error reading spilled partition chunk: {e:?}");
                                io_state.set(SpillIOState::Error);
                                None
                            }
                        },
                    );
                    let completion = Completion::new_read(read_buffer, read_complete);
                    let spill_state = self.spill_state.as_ref().expect("spill state must exist");
                    let c = spill_state.temp_file.file.pread(file_offset, completion)?;
                    if !c.finished() {
                        io_yield_one!(c);
                    }
                }
            }
        }
    }

    /// Parse entries from the current chunk buffer into buckets for a partition.
    fn parse_partition_chunk(
        &mut self,
        partition_idx: usize,
        mut metrics: Option<&mut HashJoinMetrics>,
    ) -> Result<ParseChunkResult> {
        let (has_more_chunks, resident_mem) = {
            let spill_state = self.spill_state.as_mut().expect("spill state must exist");
            let partition = spill_state
                .find_partition_mut(partition_idx)
                .expect("partition must exist for parsing");

            let data_len = partition.buffer_len();
            if let Some(metrics) = metrics.as_mut() {
                metrics.load_bytes_read = metrics.load_bytes_read.saturating_add(data_len as u64);
            }

            let data_guard = partition.read_buffer.read();
            let data = &data_guard[..data_len];
            let parse_buf = if partition.partial_entry.is_empty() {
                data.iter().copied().try_collect()?
            } else {
                let mut combined =
                    Vec::try_with_capacity_ext(partition.partial_entry.len() + data.len())?;
                combined.extend_from_slice(&partition.partial_entry);
                combined.extend_from_slice(data);
                combined
            };
            drop(data_guard);

            partition.partial_entry.clear();
            partition.buffer_len.store(0, atomic::Ordering::Release);
            partition.read_buffer.write().clear();
            partition.io_state.set(SpillIOState::None);

            let mut offset = 0;
            while offset < parse_buf.len() {
                let Some((entry_len, varint_size)) = read_varint_partial(&parse_buf[offset..])?
                else {
                    partition
                        .partial_entry
                        .try_reserve(parse_buf.len() - offset)?;
                    partition
                        .partial_entry
                        .extend_from_slice(&parse_buf[offset..]);
                    break;
                };

                let total_needed = varint_size + entry_len as usize;
                if offset + total_needed > parse_buf.len() {
                    partition
                        .partial_entry
                        .try_reserve(parse_buf.len() - offset)?;
                    partition
                        .partial_entry
                        .extend_from_slice(&parse_buf[offset..]);
                    break;
                }

                let start = offset + varint_size;
                let end = start + entry_len as usize;
                let (entry, consumed) = HashEntry::deserialize(&parse_buf[start..end])?;
                turso_assert!(
                    consumed == entry_len as usize,
                    "expected to consume entire entry"
                );

                let bucket_idx = (entry.hash as usize) % partition.buckets.len();
                partition.buckets[bucket_idx].insert(entry)?;
                partition.parsed_entries += 1;
                offset += total_needed;
            }

            partition.current_chunk_idx += 1;

            if partition.has_more_chunks() {
                (true, 0)
            } else {
                if unlikely(!partition.partial_entry.is_empty()) {
                    return Err(LimboError::Corrupt("HashEntry: truncated entry".into()));
                }
                let total_num_entries = partition.total_num_entries();
                turso_assert!(
                    partition.parsed_entries == total_num_entries,
                    "parsed entry count mismatch"
                );
                if self.track_matched && partition.matched_bits.is_empty() {
                    // Only initialize matched_bits on the first load. On subsequent
                    // reloads (after eviction), the existing bits are preserved so that
                    // probe marks set during earlier passes are not lost.
                    partition.matched_bits = partition
                        .buckets
                        .iter()
                        .map(|b| vec![false; b.entries.len()])
                        .try_collect()?;
                }
                partition.state = PartitionState::Loaded;
                partition.resident_mem = Self::partition_bucket_mem(&partition.buckets);
                // Release staging buffer to free memory now that buckets are built.
                partition.buffer_len.store(0, atomic::Ordering::SeqCst);
                partition.read_buffer.write().clear();
                (false, partition.resident_mem)
            }
        };

        if has_more_chunks {
            Ok(ParseChunkResult::MoreChunks)
        } else {
            Ok(ParseChunkResult::Done { resident_mem })
        }
    }

    /// Probe a specific partition with the given keys. The partition must be loaded first via `load_spilled_partition`.
    /// VDBE *must* call load_spilled_partition(partition_idx) and get IOResult::Done before calling probe.
    /// Returns None immediately if any probe key is NULL since NULL != NULL in SQL.
    pub fn probe_partition(
        &mut self,
        partition_idx: usize,
        probe_keys: &[Value],
        metrics: Option<&mut HashJoinMetrics>,
    ) -> Result<Option<&HashEntry>> {
        // Skip probing if any key is NULL - NULL can never match anything in SQL
        self.keep_probe_keys(probe_keys)?;
        if has_null_key(probe_keys) {
            self.current_probe_hash = None;
            return Ok(None);
        }
        let hash = hash_join_values(probe_keys, &self.collations);
        self.current_probe_hash = Some(hash);

        self.record_probe_call(metrics);
        self.touch_partition_lru(partition_idx);
        let Some(spill_state) = self.spill_state.as_ref() else {
            return Ok(None);
        };
        let Some(partition) = spill_state.find_partition(partition_idx) else {
            return Ok(None);
        };

        if !partition.is_loaded() || partition.buckets.is_empty() {
            return Ok(None);
        }

        let bucket_idx = (hash as usize) % partition.buckets.len();
        let bucket = &partition.buckets[bucket_idx];

        self.probe_bucket_idx = bucket_idx;
        self.current_spill_partition_idx = partition_idx;

        for (idx, entry) in bucket.entries.iter().enumerate() {
            if entry.hash == hash
                && keys_equal_values(&entry.key_values, probe_keys, &self.collations)
            {
                self.probe_entry_idx = idx + 1;
                return Ok(Some(entry));
            }
        }
        Ok(None)
    }

    /// AUDIT 3.1: keeps a partition probe's keys for [Self::next_match], copied into the kept buffer.
    fn keep_probe_keys(&mut self, probe_keys: &[Value]) -> Result<()> {
        refill_keys(
            self.current_probe_keys.get_or_insert_default(),
            probe_keys.iter(),
        )
    }

    /// Get the partition index for a given probe key hash.
    pub fn partition_for_keys(&self, probe_keys: &[Value]) -> Result<usize> {
        turso_assert!(
            self.spill_state.is_some(),
            "partition_for_keys requires spill state"
        );
        Ok(self.partition_index(hash_join_values(probe_keys, &self.collations)))
    }

    /// Returns true if the hash table has spilled to disk.
    pub fn has_spilled(&self) -> bool {
        self.spill_state.is_some()
    }

    /// Amphora (PL.13): the entries the table holds, and the bytes they take in memory.
    pub fn held(&self) -> (usize, usize) {
        (self.num_entries, self.mem_used)
    }

    /// Approximate memory used by a partition's buckets.
    fn partition_bucket_mem(buckets: &[HashBucket]) -> usize {
        buckets.iter().map(|b| b.size_bytes()).sum()
    }

    /// Touch a resident spilled partition for LRU ordering without changing its
    /// accounted memory.
    fn touch_partition_lru(&self, partition_idx: usize) {
        let mut lru = self.loaded_partitions_lru.borrow_mut();
        if let Some(pos) = lru.iter().position(|p| *p == partition_idx) {
            lru.remove(pos);
        }
        lru.push_back(partition_idx);
    }

    /// Record that a spilled partition is resident with the given memory
    /// footprint and update LRU ordering.
    fn record_partition_resident(&mut self, partition_idx: usize, mem_used: usize) {
        if let Some(spill_state) = self.spill_state.as_mut() {
            if let Some(partition) = spill_state.find_partition_mut(partition_idx) {
                self.loaded_partitions_mem = self
                    .loaded_partitions_mem
                    .saturating_sub(partition.resident_mem);
                partition.resident_mem = mem_used;
                self.loaded_partitions_mem += mem_used;
                self.touch_partition_lru(partition_idx);
            }
        }
    }

    fn evict_partitions_to_fit(&mut self, incoming_mem: usize, protect_idx: usize) {
        while self.mem_used + self.loaded_partitions_mem + incoming_mem > self.mem_budget {
            let Some(victim_idx) = self.next_evictable(protect_idx) else {
                break;
            };

            let mut freed = 0;
            if let Some(spill_state) = self.spill_state.as_mut() {
                if let Some(victim) = spill_state.find_partition_mut(victim_idx) {
                    if matches!(victim.state, PartitionState::Loaded) {
                        freed = victim.resident_mem;
                        victim.buckets.clear();
                        victim.state = PartitionState::OnDisk;
                        victim.resident_mem = 0;
                        victim.current_chunk_idx = 0;
                        victim.buffer_len.store(0, atomic::Ordering::Release);
                        victim.read_buffer.write().clear();
                        victim.partial_entry.clear();
                        victim.parsed_entries = 0;
                        victim.io_state.set(SpillIOState::None);
                    }
                }
            }

            self.loaded_partitions_mem = self.loaded_partitions_mem.saturating_sub(freed);
        }
    }

    /// Find the next evictable resident spilled partition (LRU) that is not
    /// protected and has backing spill data.
    fn next_evictable(&mut self, protect_idx: usize) -> Option<usize> {
        let spill_state = self.spill_state.as_ref()?;

        let len = self.loaded_partitions_lru.borrow().len();
        for i in 0..len {
            let lru = self.loaded_partitions_lru.borrow();
            let candidate = lru[i];
            if candidate == protect_idx {
                continue;
            }
            if let Some(p) = spill_state.find_partition(candidate) {
                let has_disk = !p.chunks.is_empty();
                drop(lru);
                if matches!(p.state, PartitionState::Loaded) && has_disk {
                    self.loaded_partitions_lru.borrow_mut().remove(i);
                    return Some(candidate);
                }
            }
        }
        None
    }

    /// Buffer a probe row whose target build partition is on disk.
    /// Called from op_hash_probe when `probe_rowid_reg` is Some and the
    /// partition is OnDisk.
    pub fn buffer_probe_row(
        &mut self,
        key_values: Vec<Value>,
        probe_rowid: i64,
        metrics: Option<&mut HashJoinMetrics>,
    ) -> IOResultOr<()> {
        let spill_state = self
            .spill_state
            .as_ref()
            .expect("buffer_probe_row requires build-side spill state");
        let partitioning = spill_state.partitioning;

        // Lazily initialize probe spill state on first call
        if self.probe_spill_state.is_none() {
            self.probe_spill_state = Some(ProbeSpillState::new(
                &self.io,
                self.temp_store,
                partitioning,
                self.mem_budget / 2,
            )?);
        }

        let hash = hash_join_values(&key_values, &self.collations);
        let partition_idx = partitioning.index(hash);

        let entry = HashEntry::new(hash, key_values, probe_rowid);
        let entry_size = entry.size_bytes();

        let probe_state = self
            .probe_spill_state
            .as_mut()
            .expect("probe spill state just initialized");

        probe_state.partition_buffers[partition_idx].insert(entry)?;
        probe_state.mem_used += entry_size;

        if let Some(metrics) = metrics {
            metrics.grace_probe_rows_buffered = metrics.grace_probe_rows_buffered.saturating_add(1);
        }

        // If probe buffers exceed budget, spill the largest one
        if probe_state.mem_used > probe_state.mem_budget {
            if let Some(c) = self.spill_largest_probe_partition(None)? {
                if !c.finished() {
                    return Ok(IOResult::IO(IOCompletions(c)));
                }
            }
        }

        Ok(IOResult::Done(()))
    }

    /// Spill the largest probe partition buffer to disk.
    fn spill_largest_probe_partition(
        &mut self,
        metrics: Option<&mut HashJoinMetrics>,
    ) -> Result<Option<Completion>> {
        let probe_state = self
            .probe_spill_state
            .as_mut()
            .expect("probe spill state must exist");

        let largest_idx = probe_state
            .partition_buffers
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.is_empty())
            .max_by_key(|(_, p)| p.mem_used)
            .map(|(idx, _)| idx);

        let Some(partition_idx) = largest_idx else {
            return Ok(None);
        };

        Self::spill_probe_partition(probe_state, partition_idx, &self.io, metrics)
    }

    /// Spill a probe partition buffer to its temp file.
    fn spill_probe_partition(
        probe_state: &mut ProbeSpillState,
        partition_idx: usize,
        io: &Arc<dyn IO>,
        metrics: Option<&mut HashJoinMetrics>,
    ) -> Result<Option<Completion>> {
        let partition = &probe_state.partition_buffers[partition_idx];
        if partition.is_empty() {
            return Ok(None);
        }

        // Calculate total serialized size
        let mut total_size = 0usize;
        let mut entry_sizes = Vec::try_with_capacity_ext(partition.entries.len())?;
        for entry in &partition.entries {
            let s = entry.serialized_size();
            entry_sizes.try_push(s)?;
            total_size += varint_len(s as u64) + s;
        }

        // Serialize into I/O buffer
        let buffer = Buffer::new_temporary(total_size);
        let buf = buffer.as_mut_slice();
        let mut offset = 0;
        for (entry, &entry_size) in partition.entries.iter().zip(entry_sizes.iter()) {
            offset += write_varint(&mut buf[offset..], entry_size as u64);
            offset += entry.serialize_to_slice(&mut buf[offset..]);
        }
        turso_assert!(offset == total_size, "serialized size mismatch");

        let file_offset = probe_state.next_spill_offset;
        let num_entries = partition.entries.len();
        let mem_freed = partition.mem_used;

        probe_state.partition_buffers[partition_idx].clear();

        // Record chunk
        let io_state = if let Some(existing) = probe_state.find_partition_mut(partition_idx) {
            existing.add_chunk(file_offset, total_size, num_entries)?;
            existing.io_state.clone()
        } else {
            let mut new_partition = SpilledPartition::new(partition_idx);
            new_partition.add_chunk(file_offset, total_size, num_entries)?;
            let io_state = new_partition.io_state.clone();
            probe_state.partitions.try_push(new_partition)?;
            io_state
        };

        if let Some(metrics) = metrics {
            metrics.probe_spill_bytes_written = metrics
                .probe_spill_bytes_written
                .saturating_add(total_size as u64);
            metrics.probe_spill_chunks = metrics.probe_spill_chunks.saturating_add(1);
        }

        io_state.set(SpillIOState::WaitingForWrite);
        let buffer_ref = Arc::new(buffer);
        let write_complete = Box::new(move |res: Result<i32, crate::CompletionError>| match res {
            Ok(_) => io_state.set(SpillIOState::WriteComplete),
            Err(e) => {
                tracing::error!("Error writing probe partition to disk: {e:?}");
                io_state.set(SpillIOState::Error);
            }
        });

        let completion = Completion::new_write(write_complete);
        let file = probe_state.temp_file.file.clone();
        let completion = file.pwrite(file_offset, buffer_ref, completion)?;

        probe_state.mem_used -= mem_freed;
        probe_state.next_spill_offset += total_size as u64;
        let _ = io;
        Ok(Some(completion))
    }

    /// Finalize probe-side spilling after the probe cursor is exhausted.
    /// Flushes remaining in-memory probe buffers for partitions that have spill
    /// chunks, keeps purely in-memory probe buffers as-is.
    pub fn finalize_probe_spill(
        &mut self,
        metrics: Option<&mut HashJoinMetrics>,
    ) -> IOResultOr<()> {
        let mut metrics = metrics;
        let Some(probe_state) = self.probe_spill_state.as_ref() else {
            return Ok(IOResult::Done(()));
        };

        // Wait for any pending probe writes
        for spilled in &probe_state.partitions {
            if matches!(spilled.io_state.get(), SpillIOState::WaitingForWrite) {
                io_yield_one!(Completion::new_yield());
            }
        }

        // Collect partition indices that need flushing
        let partition_count = probe_state.partitioning.count;
        let mut flush_targets = vec![];
        for partition_idx in 0..partition_count {
            if probe_state.partition_buffers[partition_idx].is_empty() {
                continue;
            }
            if probe_state.find_partition(partition_idx).is_some() {
                flush_targets.try_push(partition_idx)?;
            }
        }

        for partition_idx in flush_targets {
            if let Some(c) = Self::spill_probe_partition(
                self.probe_spill_state.as_mut().expect("probe state exists"),
                partition_idx,
                &self.io,
                metrics.as_deref_mut(),
            )? {
                if !c.finished() {
                    io_yield_one!(c);
                }
            }
        }

        Ok(IOResult::Done(()))
    }

    /// Initialize grace processing. Builds partition list, frees in-memory build partitions.
    /// Returns true if there are partitions to process. No IO.
    pub fn grace_begin(&mut self) -> Result<bool> {
        if self.probe_spill_state.is_none() || self.spill_state.is_none() {
            return Ok(false);
        }

        // Build list of partitions that were spilled on the build side
        let partitions_to_process: Vec<usize> = {
            let spill_state = self.spill_state.as_ref().expect("spill state exists");
            spill_state
                .partitions
                .iter()
                .filter(|p| !p.chunks.is_empty())
                .map(|p| p.partition_idx)
                .try_collect()?
        };

        if partitions_to_process.is_empty() {
            return Ok(false);
        }

        // Free in-memory build partitions -- initial probe is done with them
        self.free_in_memory_build_partitions();

        self.grace_state = Some(GraceState {
            probe_entries: vec![],
            probe_entry_cursor: 0,
            partitions_to_process,
            partition_list_idx: 0,
            load_state: GracePartitionLoadState::NeedBuildLoad,
        });

        if let Some(probe_state) = self.probe_spill_state.as_mut() {
            for partition in &mut probe_state.partitions {
                partition.current_chunk_idx = 0;
                partition.buffer_len.store(0, atomic::Ordering::Release);
                partition.read_buffer.write().clear();
                partition.partial_entry.clear();
                partition.parsed_entries = 0;
                partition.io_state.set(SpillIOState::None);
            }
        }

        self.state = HashTableState::GraceProcessing;
        Ok(true)
    }

    /// Load build partition at current index + first probe chunk. IO-blocking.
    /// Returns true if loaded, false if partition list exhausted.
    pub fn grace_load_current_partition(
        &mut self,
        mut metrics: Option<&mut HashJoinMetrics>,
    ) -> IOResultOr<bool> {
        loop {
            let grace = self.grace_state.as_ref().expect("grace state must exist");
            if grace.partition_list_idx >= grace.partitions_to_process.len() {
                return Ok(IOResult::Done(false));
            }

            let partition_idx = grace.partitions_to_process[grace.partition_list_idx];
            match grace.load_state {
                GracePartitionLoadState::NeedBuildLoad => {
                    self.evict_all_loaded_partitions();
                    return_if_io!(
                        self.load_spilled_partition(partition_idx, metrics.as_deref_mut())
                    );

                    let grace = self.grace_state.as_mut().expect("grace state");
                    grace.probe_entries.clear();
                    grace.probe_entry_cursor = 0;
                    grace.load_state = GracePartitionLoadState::NeedProbeLoad;
                }
                GracePartitionLoadState::NeedProbeLoad => {
                    return_if_io!(self.grace_load_probe_entries(partition_idx));

                    let grace = self.grace_state.as_mut().expect("grace state");
                    grace.load_state = GracePartitionLoadState::Ready;
                    if let Some(m) = metrics.as_mut() {
                        m.grace_partitions_processed =
                            m.grace_partitions_processed.saturating_add(1);
                    }
                    return Ok(IOResult::Done(true));
                }
                GracePartitionLoadState::Ready => return Ok(IOResult::Done(true)),
            }
        }
    }

    /// Advance to next probe entry. Returns keys+rowid or None when exhausted.
    pub fn grace_next_probe_entry(&mut self) -> IOResultOr<Option<GraceProbeEntry>> {
        loop {
            let grace = self.grace_state.as_ref().expect("grace state must exist");
            if grace.probe_entry_cursor < grace.probe_entries.len() {
                let entry = &grace.probe_entries[grace.probe_entry_cursor];
                let result = GraceProbeEntry {
                    key_values: entry.key_values.iter().cloned().try_collect()?,
                    probe_rowid: entry.rowid,
                };
                let grace = self.grace_state.as_mut().expect("grace state");
                grace.probe_entry_cursor += 1;
                return Ok(IOResult::Done(Some(result)));
            }

            // Current probe entries exhausted, try loading more
            let grace = self.grace_state.as_ref().expect("grace state");
            let partition_idx = match grace.current_partition_idx() {
                Some(idx) => idx,
                None => return Ok(IOResult::Done(None)),
            };
            match self.grace_try_load_next_probe_chunk(partition_idx)? {
                IOResult::Done(true) => continue,
                IOResult::Done(false) => return Ok(IOResult::Done(None)),
                IOResult::IO(io) => return Ok(IOResult::IO(io)),
            }
        }
    }

    /// Try to load the next probe chunk for the given partition.
    /// Returns true if more probe entries were loaded, false if exhausted.
    fn grace_try_load_next_probe_chunk(&mut self, partition_idx: usize) -> IOResultOr<bool> {
        loop {
            let Some(probe_state) = self.probe_spill_state.as_ref() else {
                return Ok(IOResult::Done(false));
            };

            let Some(spilled) = probe_state.find_partition(partition_idx) else {
                return Ok(IOResult::Done(false));
            };

            if spilled.current_chunk_idx >= spilled.chunks.len() {
                return Ok(IOResult::Done(false));
            }

            match self.grace_load_next_probe_chunk(partition_idx)? {
                IOResult::Done(true) => return Ok(IOResult::Done(true)),
                IOResult::Done(false) => continue,
                IOResult::IO(io) => return Ok(IOResult::IO(io)),
            }
        }
    }

    /// Evict current partition, advance to next. Returns true if more partitions. No IO.
    pub fn grace_advance_partition(&mut self) -> bool {
        self.evict_all_loaded_partitions();
        let grace = self.grace_state.as_mut().expect("grace state must exist");
        grace.partition_list_idx += 1;
        grace.probe_entries.clear();
        grace.probe_entry_cursor = 0;
        grace.load_state = GracePartitionLoadState::NeedBuildLoad;
        grace.partition_list_idx < grace.partitions_to_process.len()
    }

    /// Free all in-memory build partitions (InMemory state).
    fn free_in_memory_build_partitions(&mut self) {
        if let Some(spill_state) = self.spill_state.as_mut() {
            for partition in &mut spill_state.partitions {
                if matches!(partition.state, PartitionState::InMemory) {
                    partition.buckets.clear();
                    partition.state = PartitionState::OnDisk;
                    partition.resident_mem = 0;
                }
            }
        }
        // Also free the main slab
        self.heads.clear();
        self.entries.clear();
        self.next.clear();
        self.matched_bits.clear();
        self.non_empty_buckets.clear();
        self.loaded_partitions_lru.borrow_mut().clear();
        self.loaded_partitions_mem = 0;
    }

    /// Evict all currently loaded build partitions.
    fn evict_all_loaded_partitions(&mut self) {
        if let Some(spill_state) = self.spill_state.as_mut() {
            for partition in &mut spill_state.partitions {
                if matches!(partition.state, PartitionState::Loaded) && !partition.chunks.is_empty()
                {
                    partition.buckets.clear();
                    partition.state = PartitionState::OnDisk;
                    partition.resident_mem = 0;
                    partition.current_chunk_idx = 0;
                    partition.buffer_len.store(0, atomic::Ordering::Release);
                    partition.read_buffer.write().clear();
                    partition.partial_entry.clear();
                    partition.parsed_entries = 0;
                    partition.io_state.set(SpillIOState::None);
                }
            }
        }
        self.loaded_partitions_lru.borrow_mut().clear();
        self.loaded_partitions_mem = 0;
    }

    /// Load probe entries for a given partition into grace_state.probe_entries.
    /// Loads from in-memory buffers or from the first spill chunk.
    fn grace_load_probe_entries(&mut self, partition_idx: usize) -> IOResultOr<()> {
        {
            let grace = self.grace_state.as_mut().expect("grace state");
            grace.probe_entries.clear();
            grace.probe_entry_cursor = 0;
        }

        let Some(probe_state) = self.probe_spill_state.as_ref() else {
            return Ok(IOResult::Done(()));
        };

        // First: check if there are in-memory entries for this partition
        let buffer = &probe_state.partition_buffers[partition_idx];
        if !buffer.is_empty() {
            let grace = self.grace_state.as_mut().expect("grace state");
            grace.probe_entries.clone_from(&buffer.entries);
            return Ok(IOResult::Done(()));
        }

        // Check if there are spill chunks
        let Some(spilled) = probe_state.find_partition(partition_idx) else {
            return Ok(IOResult::Done(()));
        };
        if spilled.chunks.is_empty() {
            return Ok(IOResult::Done(()));
        }

        self.grace_load_next_probe_chunk(partition_idx)
            .map(|result| result.map(|_| ()))
    }

    /// Load the next probe spill chunk into grace_state.probe_entries.
    fn grace_load_next_probe_chunk(&mut self, partition_idx: usize) -> IOResultOr<bool> {
        loop {
            let action = {
                let probe_state = self.probe_spill_state.as_mut().expect("probe spill state");
                let spilled = probe_state
                    .find_partition_mut(partition_idx)
                    .expect("probe partition must exist");
                let io_state = spilled.io_state.get();

                if unlikely(matches!(io_state, SpillIOState::Error)) {
                    return Err(
                        LimboError::InternalError("grace probe spill I/O failure".into()).into(),
                    );
                }

                if matches!(io_state, SpillIOState::WaitingForRead) {
                    GraceProbeChunkAction::WaitingForIO
                } else if matches!(io_state, SpillIOState::ReadComplete) {
                    GraceProbeChunkAction::ParseChunk { partition_idx }
                } else {
                    match spilled.current_chunk() {
                        Some(chunk) if chunk.size_bytes == 0 => {
                            spilled.current_chunk_idx += 1;
                            GraceProbeChunkAction::Restart
                        }
                        Some(chunk) => {
                            spilled.io_state.set(SpillIOState::WaitingForRead);
                            GraceProbeChunkAction::LoadChunk {
                                read_size: chunk.size_bytes,
                                file_offset: chunk.file_offset,
                                io_state: spilled.io_state.clone(),
                                buffer_len: spilled.buffer_len.clone(),
                                read_buffer_ref: spilled.read_buffer.clone(),
                            }
                        }
                        None => GraceProbeChunkAction::NoMoreChunks,
                    }
                }
            };

            match action {
                GraceProbeChunkAction::WaitingForIO => {
                    io_yield_one!(Completion::new_yield());
                }
                GraceProbeChunkAction::ParseChunk { partition_idx } => {
                    return Ok(IOResult::Done(self.parse_grace_probe_chunk(partition_idx)?));
                }
                GraceProbeChunkAction::LoadChunk {
                    read_size,
                    file_offset,
                    io_state,
                    buffer_len,
                    read_buffer_ref,
                } => {
                    let read_buffer = Arc::new(Buffer::new_temporary(read_size));
                    let read_complete = Box::new(
                        move |res: Result<(Arc<Buffer>, i32), CompletionError>| match res {
                            Ok((buf, bytes_read)) => {
                                let mut persistent_buf = read_buffer_ref.write();
                                persistent_buf.clear();
                                persistent_buf
                                    .extend_from_slice(&buf.as_slice()[..bytes_read as usize]);
                                buffer_len.store(bytes_read as usize, atomic::Ordering::Release);
                                io_state.set(SpillIOState::ReadComplete);
                                None
                            }
                            Err(e) => {
                                mark_unlikely();
                                tracing::error!("Error reading probe chunk: {e:?}");
                                io_state.set(SpillIOState::Error);
                                None
                            }
                        },
                    );

                    let completion = Completion::new_read(read_buffer, read_complete);
                    let probe_state = self.probe_spill_state.as_ref().expect("probe spill state");
                    let c = probe_state.temp_file.file.pread(file_offset, completion)?;
                    if !c.finished() {
                        io_yield_one!(c);
                    }
                }
                GraceProbeChunkAction::Restart => continue,
                GraceProbeChunkAction::NoMoreChunks => return Ok(IOResult::Done(false)),
            }
        }
    }

    fn parse_grace_probe_chunk(&mut self, partition_idx: usize) -> Result<bool> {
        let entries = {
            let probe_state = self.probe_spill_state.as_mut().expect("probe spill state");
            let partition = probe_state
                .find_partition_mut(partition_idx)
                .expect("probe partition must exist for parsing");
            let chunk = partition
                .current_chunk()
                .expect("probe chunk must exist while parsing");
            let expected_entries = chunk.num_entries;
            let data_len = partition.buffer_len();

            let data_guard = partition.read_buffer.read();
            let data = &data_guard[..data_len];
            let mut entries = Vec::try_with_capacity_ext(expected_entries)?;
            let mut offset = 0;
            while offset < data_len {
                let Some((entry_len, varint_size)) = read_varint_partial(&data[offset..])? else {
                    return Err(LimboError::InternalError(
                        "truncated grace probe spill chunk header".into(),
                    ));
                };
                let total_needed = varint_size + entry_len as usize;
                if offset + total_needed > data_len {
                    return Err(LimboError::InternalError(
                        "truncated grace probe spill chunk payload".into(),
                    ));
                }
                let start = offset + varint_size;
                let end = start + entry_len as usize;
                let (entry, _consumed) = HashEntry::deserialize(&data[start..end])?;
                entries.try_push(entry)?;
                offset += total_needed;
            }
            drop(data_guard);

            if unlikely(entries.len() != expected_entries) {
                return Err(LimboError::InternalError(format!(
                    "grace probe spill chunk entry count mismatch: expected {expected_entries}, got {}",
                    entries.len()
                )));
            }

            partition.buffer_len.store(0, atomic::Ordering::Release);
            partition.read_buffer.write().clear();
            partition.io_state.set(SpillIOState::None);
            partition.current_chunk_idx += 1;
            entries
        };

        let grace = self.grace_state.as_mut().expect("grace state");
        grace.probe_entries = entries;
        grace.probe_entry_cursor = 0;
        Ok(!grace.probe_entries.is_empty())
    }

    /// Returns true if grace processing has any spilled partitions to process.
    pub fn has_grace_partitions(&self) -> bool {
        self.probe_spill_state.is_some()
    }

    /// Close the hash table and free resources.
    pub fn close(&mut self) {
        self.state = HashTableState::Closed;
        self.heads.clear();
        self.entries.clear();
        self.next.clear();
        self.matched_bits.clear();
        self.non_empty_buckets.clear();
        self.num_entries = 0;
        self.mem_used = 0;
        self.loaded_partitions_lru.borrow_mut().clear();
        self.loaded_partitions_mem = 0;
        let _ = self.spill_state.take();
        self.probe_spill_state = None;
        self.grace_state = None;
    }
}

#[cfg(test)]
mod hashtests {
    use super::*;
    use crate::alloc::vec;
    use crate::io::Buffer;
    use crate::MemoryIO;

    /// The allocations a thread makes while it counts (AUDIT 3.1's probe test).
    mod counting {
        use std::alloc::{GlobalAlloc, Layout, System};
        use std::cell::Cell;

        struct Counting;

        thread_local! {
            static COUNTING: Cell<bool> = const { Cell::new(false) };
            static COUNT: Cell<u64> = const { Cell::new(0) };
        }

        unsafe impl GlobalAlloc for Counting {
            unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
                let _ = COUNTING.try_with(|on| {
                    if on.get() {
                        let _ = COUNT.try_with(|n| n.set(n.get() + 1));
                    }
                });
                unsafe { System.alloc(layout) }
            }

            unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
                let _ = COUNTING.try_with(|on| {
                    if on.get() {
                        let _ = COUNT.try_with(|n| n.set(n.get() + 1));
                    }
                });
                unsafe { System.realloc(ptr, layout, new_size) }
            }

            unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
                unsafe { System.dealloc(ptr, layout) }
            }
        }

        #[global_allocator]
        static ALLOC: Counting = Counting;

        pub(super) fn counted<T>(work: impl FnOnce() -> T) -> (u64, T) {
            COUNT.with(|n| n.set(0));
            COUNTING.with(|on| on.set(true));
            let out = work();
            COUNTING.with(|on| on.set(false));
            (COUNT.with(|n| n.get()), out)
        }
    }

    /// AUDIT 3.1: once the probe's key buffer has held a key its size, a probe row allocates
    /// nothing: not its keys (integer and NOCASE text), not their hash, not a hit, a miss, or any
    /// further match.
    #[test]
    fn audit_3_1_probe_rows_allocate_nothing() {
        let io = Arc::new(MemoryIO::new());
        let config = HashTableConfig {
            initial_buckets: 4,
            mem_budget: 64 * 1024 * 1024,
            num_keys: 2,
            collations: vec![CollationSeq::Binary, CollationSeq::NoCase],
            temp_store: crate::TempStore::Memory,
            track_matched: false,
            partition_count: None,
        };
        let mut ht = HashTable::new(config, io).unwrap();
        let key = |i: i64| [Value::from_i64(i), Value::from_text(format!("Key-{i}"))];
        for i in 0..1000i64 {
            let _ = ht
                .insert(key(i % 500).to_vec(), i + 1, vec![], None)
                .unwrap();
        }
        let _ = ht.finalize_build(None);
        let probes: std::vec::Vec<[Value; 2]> = (0..1000).map(key).collect();
        let probe = |ht: &mut HashTable, k: &[Value]| -> usize {
            let keys = ht.probe_keys_from(k.iter()).unwrap();
            let mut matched = usize::from(ht.probe(keys, None).unwrap().is_some());
            while ht.next_match().unwrap().is_some() {
                matched += 1;
            }
            matched
        };
        assert_eq!(probe(&mut ht, &probes[999]), 0);
        let (allocations, matched) =
            counting::counted(|| probes.iter().map(|k| probe(&mut ht, k)).sum::<usize>());
        assert_eq!(matched, 1000, "each of keys 0-499 twice, 500-999 never");
        assert_eq!(allocations, 0, "allocations over 1,000 probe rows");
    }

    #[test]
    fn test_hash_table_rejects_custom_collations() {
        let io = Arc::new(MemoryIO::new());
        let config = HashTableConfig {
            collations: vec![CollationSeq::custom("hash_table_custom")],
            ..Default::default()
        };
        let err = match HashTable::new(config, io) {
            Ok(_) => panic!("custom-collated hash table should be rejected"),
            Err(err) => err,
        };
        assert!(err
            .to_string()
            .contains("custom collations are not supported by hash tables"));
    }

    #[test]
    fn test_hash_function_consistency() {
        // Test that the same keys produce the same hash
        let keys1 = vec![
            ValueRef::from_i64(42),
            ValueRef::Text(crate::types::TextRef::new(
                "hello",
                crate::types::TextSubtype::Text,
            )),
        ];
        let keys2 = vec![
            ValueRef::from_i64(42),
            ValueRef::Text(crate::types::TextRef::new(
                "hello",
                crate::types::TextSubtype::Text,
            )),
        ];
        let keys3 = vec![
            ValueRef::from_i64(43),
            ValueRef::Text(crate::types::TextRef::new(
                "hello",
                crate::types::TextSubtype::Text,
            )),
        ];

        let collations = vec![CollationSeq::Binary, CollationSeq::Binary];
        let hash1 = hash_join_key(&keys1, &collations);
        let hash2 = hash_join_key(&keys2, &collations);
        let hash3 = hash_join_key(&keys3, &collations);

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_hash_function_numeric_equivalence() {
        let collations = vec![CollationSeq::Binary];

        // Zero variants should hash identically
        let h_zero = hash_join_key(&[ValueRef::from_f64(0.0)], &collations);
        let h_neg_zero = hash_join_key(&[ValueRef::from_f64(-0.0)], &collations);
        let h_int_zero = hash_join_key(&[ValueRef::from_i64(0)], &collations);
        assert_eq!(h_zero, h_neg_zero);
        assert_eq!(h_zero, h_int_zero);

        // Integer/float representations of the same numeric value should match
        let h_ten_int = hash_join_key(&[ValueRef::from_i64(10)], &collations);
        let h_ten_float = hash_join_key(&[ValueRef::from_f64(10.0)], &collations);
        assert_eq!(h_ten_int, h_ten_float);

        let h_neg_ten_int = hash_join_key(&[ValueRef::from_i64(-10)], &collations);
        let h_neg_ten_float = hash_join_key(&[ValueRef::from_f64(-10.0)], &collations);
        assert_eq!(h_neg_ten_int, h_neg_ten_float);

        // Positive/negative values should still differ
        assert_ne!(h_ten_int, h_neg_ten_int);
    }

    #[test]
    fn test_keys_equal() {
        let key1 = vec![Value::from_i64(42), Value::Text("hello".to_string().into())];
        let key2 = vec![
            ValueRef::from_i64(42),
            ValueRef::Text(crate::types::TextRef::new(
                "hello",
                crate::types::TextSubtype::Text,
            )),
        ];
        let key3 = vec![
            ValueRef::from_i64(43),
            ValueRef::Text(crate::types::TextRef::new(
                "hello",
                crate::types::TextSubtype::Text,
            )),
        ];

        let collations = vec![CollationSeq::Binary, CollationSeq::Binary];
        assert!(keys_equal(&key1, &key2, &collations));
        assert!(!keys_equal(&key1, &key3, &collations));
    }

    /// AUDIT 3.1: the bucket array doubles as rows arrive, keeping every row findable, the
    /// chains at most a load factor of 1, and a FULL OUTER join's matched bits beside their rows.
    #[test]
    fn audit_3_1_buckets_grow_and_keep_every_row() {
        for track_matched in [false, true] {
            let io = Arc::new(MemoryIO::new());
            let config = HashTableConfig {
                initial_buckets: 4,
                mem_budget: 64 * 1024 * 1024,
                num_keys: 1,
                collations: vec![CollationSeq::Binary],
                temp_store: crate::TempStore::Memory,
                track_matched,
                partition_count: None,
            };
            let mut ht = HashTable::new(config, io).unwrap();
            let n = 10_000i64;
            for i in 0..n {
                let _ = ht
                    .insert(vec![Value::from_i64(i)], i + 1, vec![], None)
                    .unwrap();
            }
            assert!(!ht.has_spilled());
            assert!(ht.heads.len() >= n as usize, "{}", ht.heads.len());
            assert!(ht.heads.len().is_power_of_two());
            assert!(ht.bucket_mem > 0);
            if track_matched {
                assert_eq!(ht.matched_bits.len(), ht.entries.len());
            }
            let _ = ht.finalize_build(None);
            for i in (0..n).step_by(97) {
                let entry = ht.probe(vec![Value::from_i64(i)], None).unwrap().unwrap();
                assert_eq!(entry.rowid, i + 1);
            }
            assert!(ht.probe(vec![Value::from_i64(n)], None).unwrap().is_none());
        }
    }

    /// AUDIT 3.1: a table whose bigger array wouldn't fit its budget keeps its size, so growing
    /// never passes the memory bound; DISTINCT still sees each key once across a growth.
    #[test]
    fn audit_3_1_growth_stays_within_budget_and_distinct_holds() {
        let io = Arc::new(MemoryIO::new());
        let config = HashTableConfig {
            initial_buckets: 4,
            mem_budget: 1024 * 1024,
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Memory,
            track_matched: false,
            partition_count: None,
        };
        let mut ht = HashTable::new(config, io).unwrap();
        let mut inserted = 0;
        for round in 0..2 {
            for i in 0..2_000i64 {
                let key = [Value::from_i64(i)];
                let refs = [key[0].as_ref()];
                if let IOResult::Done(true) = ht.insert_distinct(&key, &refs, None).unwrap() {
                    inserted += 1;
                }
            }
            assert_eq!(inserted, 2_000, "round {round}");
            assert!(ht.mem_used + ht.bucket_mem <= ht.mem_budget);
        }
        assert!(ht.heads.len() >= 2_000);
        let small = HashTableConfig {
            initial_buckets: 4,
            mem_budget: 4 * 1024,
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Memory,
            track_matched: false,
            partition_count: None,
        };
        let mut ht = HashTable::new(small, Arc::new(MemoryIO::new())).unwrap();
        for i in 0..64i64 {
            let _ = ht
                .insert(vec![Value::from_i64(i)], i, vec![], None)
                .unwrap();
            assert!(ht.has_spilled() || ht.mem_used + ht.bucket_mem <= ht.mem_budget);
        }
    }

    #[test]
    fn test_hash_table_basic() {
        let io = Arc::new(MemoryIO::new());
        let config = HashTableConfig {
            initial_buckets: 4,
            mem_budget: 1024 * 1024,
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Default,
            track_matched: false,
            partition_count: None,
        };
        let mut ht = HashTable::new(config, io).unwrap();

        // Insert some entries (late materialization - only store rowids)
        let key1 = vec![Value::from_i64(1)];
        let _ = ht.insert(key1.clone(), 100, vec![], None).unwrap();

        let key2 = vec![Value::from_i64(2)];
        let _ = ht.insert(key2.clone(), 200, vec![], None).unwrap();

        let _ = ht.finalize_build(None);
        let mut metrics = HashJoinMetrics::default();

        // Probe for key1
        let result = ht.probe(key1, Some(&mut metrics)).unwrap();
        assert!(result.is_some());
        let entry1 = result.unwrap();
        assert_eq!(entry1.key_values[0].as_ref(), ValueRef::from_i64(1));
        assert_eq!(entry1.rowid, 100);

        // Probe for key2
        let result = ht.probe(key2, Some(&mut metrics)).unwrap();
        assert!(result.is_some());
        let entry2 = result.unwrap();
        assert_eq!(entry2.key_values[0].as_ref(), ValueRef::from_i64(2));
        assert_eq!(entry2.rowid, 200);

        // Probe for non-existent key
        let result = ht
            .probe(vec![Value::from_i64(999)], Some(&mut metrics))
            .unwrap();
        assert!(result.is_none());
        assert_eq!(metrics.probe_calls, 3);
    }

    #[test]
    fn test_hash_table_collisions() {
        let io = Arc::new(MemoryIO::new());
        let config = HashTableConfig {
            initial_buckets: 2, // Small number to force collisions
            mem_budget: 1024 * 1024,
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Default,
            track_matched: false,
            partition_count: None,
        };
        let mut ht = HashTable::new(config, io).unwrap();

        // Insert multiple entries (late materialization - only store rowids)
        for i in 0..10 {
            let key = vec![Value::from_i64(i)];
            let _ = ht.insert(key, i * 100, vec![], None).unwrap();
        }

        let _ = ht.finalize_build(None);

        // Verify all entries can be found
        for i in 0..10 {
            let result = ht.probe(vec![Value::from_i64(i)], None).unwrap();
            assert!(result.is_some());
            let entry = result.unwrap();
            assert_eq!(entry.key_values[0].as_ref(), ValueRef::from_i64(i));
            assert_eq!(entry.rowid, i * 100);
        }
    }

    #[test]
    fn test_hash_table_duplicate_keys() {
        let io = Arc::new(MemoryIO::new());
        let config = HashTableConfig {
            initial_buckets: 4,
            mem_budget: 1024 * 1024,
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Default,
            track_matched: false,
            partition_count: None,
        };
        let mut ht = HashTable::new(config, io).unwrap();

        // Insert multiple entries with the same key
        let key = vec![Value::from_i64(42)];
        for i in 0..3 {
            let _ = ht.insert(key.clone(), 1000 + i, vec![], None).unwrap();
        }

        let _ = ht.finalize_build(None);

        // Probe should return first match
        let result = ht.probe(key, None).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().rowid, 1000);

        // next_match should return additional matches
        let result2 = ht.next_match().unwrap();
        assert!(result2.is_some());
        assert_eq!(result2.unwrap().rowid, 1001);

        let result3 = ht.next_match().unwrap();
        assert!(result3.is_some());
        assert_eq!(result3.unwrap().rowid, 1002);

        // No more matches
        let result4 = ht.next_match().unwrap();
        assert!(result4.is_none());
    }

    /// QT.5 step 4 (lane qs5-21, T-QT.5.18): a sort-merge join's table finds what the hash
    /// table finds. Do: put the same rows (repeated keys, NULLs, 1 and 1.0, text, a blob) in a
    /// hash table and a merge's, probe both in key order, then out of it, then mark and scan the
    /// unmatched rows. Expect: the same matches for every probe key, merging forward for the
    /// ordered probes and by binary search for the rest, and the same unmatched rows.
    #[test]
    fn qt_5_18_merge_table_finds_what_the_hash_table_finds() {
        let keys = [
            Value::from_i64(3),
            Value::Null,
            Value::from_i64(1),
            Value::from_f64(1.0),
            Value::from_i64(3),
            Value::from_text("b"),
            Value::from_i64(7),
            Value::from_text("a"),
            Value::from_i64(3),
            Value::from_slice(&[1, 2]).unwrap(),
            Value::from_f64(2.5),
        ];
        let table = |merge: bool| {
            let config = HashTableConfig {
                initial_buckets: 4,
                mem_budget: 1024 * 1024,
                num_keys: 1,
                collations: vec![CollationSeq::Binary],
                temp_store: crate::TempStore::Default,
                track_matched: true,
                partition_count: None,
            };
            let mut ht = HashTable::new(config, Arc::new(MemoryIO::new())).unwrap();
            ht.set_merge(merge);
            for (rowid, key) in (0_i64..).zip(&keys) {
                let _ = ht.insert(vec![key.clone()], rowid, vec![], None).unwrap();
            }
            let _ = ht.finalize_build(None).unwrap();
            ht
        };
        let (mut hash, mut merge) = (table(false), table(true));
        assert!(!merge.merge_order.is_empty(), "the merge's rows are sorted");
        let matches = |ht: &mut HashTable, key: &Value, metrics: &mut HashJoinMetrics| {
            let mut rowids = vec![];
            let mut found = ht
                .probe(vec![key.clone()], Some(metrics))
                .unwrap()
                .map(|e| e.rowid);
            while let Some(rowid) = found {
                rowids.push(rowid);
                ht.mark_current_matched();
                found = ht.next_match().unwrap().map(|e| e.rowid);
            }
            rowids.sort_unstable();
            rowids
        };
        let ordered = [
            Value::from_i64(0),
            Value::from_i64(1),
            Value::from_i64(1),
            Value::from_f64(2.5),
            Value::from_i64(3),
            Value::from_i64(3),
            Value::from_text("a"),
            Value::from_text("c"),
        ];
        let (mut hash_metrics, mut merge_metrics) =
            (HashJoinMetrics::default(), HashJoinMetrics::default());
        for key in &ordered {
            assert_eq!(
                matches(&mut merge, key, &mut merge_metrics),
                matches(&mut hash, key, &mut hash_metrics),
                "{key:?} in key order"
            );
        }
        assert_eq!(merge_metrics.merge_searches, 0, "an ordered probe merges");
        assert!(merge_metrics.merge_steps > 0);
        for key in [
            Value::from_i64(1),
            Value::Null,
            Value::from_slice(&[1, 2]).unwrap(),
            Value::from_i64(3),
        ] {
            assert_eq!(
                matches(&mut merge, &key, &mut merge_metrics),
                matches(&mut hash, &key, &mut hash_metrics),
                "{key:?} out of key order"
            );
        }
        assert!(
            merge_metrics.merge_searches > 0,
            "a probe out of order searches"
        );
        let unmatched = |ht: &mut HashTable| {
            ht.begin_unmatched_scan();
            let mut rowids = vec![];
            while let Some(entry) = ht.next_unmatched() {
                rowids.push(entry.rowid);
            }
            rowids.sort_unstable();
            rowids
        };
        assert_eq!(unmatched(&mut merge), unmatched(&mut hash));
    }

    #[test]
    fn test_hash_entry_serialization() {
        // Test that entries serialize and deserialize correctly
        let entry = HashEntry::new(
            12345,
            vec![
                Value::from_i64(42),
                Value::Text("hello".to_string().into()),
                Value::Null,
                Value::from_f64(std::f64::consts::PI),
            ],
            100,
        );

        let mut buf = vec![];
        entry.serialize(&mut buf).unwrap();

        let (deserialized, consumed) = HashEntry::deserialize(&buf).unwrap();
        assert_eq!(consumed, buf.len());
        assert_eq!(deserialized.hash, entry.hash);
        assert_eq!(deserialized.rowid, entry.rowid);
        assert_eq!(deserialized.key_values.len(), entry.key_values.len());

        for (v1, v2) in deserialized.key_values.iter().zip(entry.key_values.iter()) {
            match (v1, v2) {
                (Value::Numeric(Numeric::Integer(i1)), Value::Numeric(Numeric::Integer(i2))) => {
                    assert_eq!(i1, i2)
                }
                (Value::Text(t1), Value::Text(t2)) => assert_eq!(t1.as_str(), t2.as_str()),
                (Value::Numeric(Numeric::Float(f1)), Value::Numeric(Numeric::Float(f2))) => {
                    assert!((f64::from(*f1) - f64::from(*f2)).abs() < 1e-10)
                }
                (Value::Null, Value::Null) => {}
                _ => panic!("Value type mismatch"),
            }
        }
    }

    #[test]
    fn test_serialize_to_slice_matches_serialize() {
        // Test that serialize_to_slice produces identical output to serialize
        let entry = HashEntry::new_with_payload(
            12345,
            vec![
                Value::from_i64(42),
                Value::Text("hello world".to_string().into()),
                Value::Null,
                Value::from_f64(std::f64::consts::PI),
            ],
            100,
            vec![
                Value::from_slice(&[1, 2, 3, 4, 5]).expect(crate::alloc::ALLOC_ERR_MSG),
                Value::from_i64(-999),
            ],
        );

        // Serialize using the Vec-based method
        let mut vec_buf = vec![];
        entry.serialize(&mut vec_buf).unwrap();

        // Serialize using the slice-based method
        let size = entry.serialized_size();
        assert_eq!(
            size,
            vec_buf.len(),
            "serialized_size must match actual size"
        );

        let mut slice_buf = vec![0u8; size];
        let written = entry.serialize_to_slice(&mut slice_buf);
        assert_eq!(written, size, "bytes written must match serialized_size");

        // Both methods must produce identical output
        assert_eq!(
            vec_buf, slice_buf,
            "serialize and serialize_to_slice must produce identical output"
        );

        // Verify the output is valid by deserializing
        let (deserialized, consumed) = HashEntry::deserialize(&slice_buf).unwrap();
        assert_eq!(consumed, size);
        assert_eq!(deserialized.hash, entry.hash);
        assert_eq!(deserialized.rowid, entry.rowid);
        assert_eq!(deserialized.key_values.len(), entry.key_values.len());
        assert_eq!(
            deserialized.payload_values.len(),
            entry.payload_values.len()
        );
    }

    #[test]
    fn test_partition_from_hash() {
        // Test partition distribution
        let partitioning = Partitioning::new(16);
        let mut counts = [0usize; 16];
        for i in 0u64..10000 {
            let hash = i.wrapping_mul(0x9E3779B97F4A7C15); // Simple hash spreading
            let partition = partitioning.index(hash);
            assert!(partition < counts.len());
            counts[partition] += 1;
        }

        // Check reasonable distribution (each partition should have some entries)
        for count in counts {
            assert!(count > 0, "Each partition should have some entries");
        }
    }

    #[test]
    fn test_spill_chunk_tracking() {
        // Test that SpilledPartition can track multiple chunks
        let mut partition = SpilledPartition::new(5);
        assert_eq!(partition.partition_idx, 5);
        assert!(partition.chunks.is_empty());
        assert_eq!(partition.total_size_bytes(), 0);
        assert_eq!(partition.total_num_entries(), 0);

        // Add first chunk
        partition.add_chunk(0, 1000, 50).unwrap();
        assert_eq!(partition.chunks.len(), 1);
        assert_eq!(partition.total_size_bytes(), 1000);
        assert_eq!(partition.total_num_entries(), 50);

        // Add second chunk
        partition.add_chunk(1000, 500, 25).unwrap();
        assert_eq!(partition.chunks.len(), 2);
        assert_eq!(partition.total_size_bytes(), 1500);
        assert_eq!(partition.total_num_entries(), 75);

        // Check individual chunks
        assert_eq!(partition.chunks[0].file_offset, 0);
        assert_eq!(partition.chunks[0].size_bytes, 1000);
        assert_eq!(partition.chunks[1].file_offset, 1000);
        assert_eq!(partition.chunks[1].size_bytes, 500);
    }

    #[test]
    fn test_partition_count_override() {
        let io = Arc::new(MemoryIO::new());
        let config = HashTableConfig {
            initial_buckets: 4,
            mem_budget: 1024,
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Default,
            track_matched: false,
            partition_count: Some(64),
        };
        let mut ht = HashTable::new(config, io).unwrap();
        insert_many_force_spill(&mut ht, 0, 1024);
        let _ = ht.finalize_build(None).unwrap();
        assert!(ht.has_spilled());

        let spill_state = ht.spill_state.as_ref().expect("spill state exists");
        assert_eq!(spill_state.partitioning.count, 64);
    }

    #[test]
    fn test_adaptive_partition_count_bounds() {
        let io = Arc::new(MemoryIO::new());
        let config = HashTableConfig {
            initial_buckets: 4,
            mem_budget: 1024,
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Default,
            track_matched: false,
            partition_count: None,
        };
        let mut ht = HashTable::new(config, io).unwrap();
        insert_many_force_spill(&mut ht, 0, 1024);
        let _ = ht.finalize_build(None).unwrap();
        assert!(ht.has_spilled());

        let spill_state = ht.spill_state.as_ref().expect("spill state exists");
        let count = spill_state.partitioning.count;
        assert!(count.is_power_of_two());
        assert!(count >= MIN_PARTITIONS);
        assert!(count <= MAX_PARTITIONS);
    }

    #[test]
    fn test_spill_streaming_parse_multiple_chunks() {
        let io = Arc::new(MemoryIO::new());
        let config = HashTableConfig {
            initial_buckets: 4,
            mem_budget: 1024,
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Default,
            track_matched: false,
            partition_count: Some(16),
        };
        let mut ht = HashTable::new(config, io).unwrap();

        let key = vec![Value::from_i64(1)];
        for i in 0..2048 {
            match ht.insert(key.clone(), i, vec![], None).unwrap() {
                IOResult::Done(()) => {}
                IOResult::IO(_) => panic!("memory IO"),
            }
        }

        match ht.finalize_build(None).unwrap() {
            IOResult::Done(()) => {}
            IOResult::IO(_) => panic!("memory IO"),
        }
        assert!(ht.has_spilled());

        let partition_idx = ht.partition_for_keys(&key).unwrap();
        {
            let spill_state = ht.spill_state.as_ref().expect("spill state exists");
            let partition = spill_state
                .find_partition(partition_idx)
                .expect("partition exists");
            assert!(partition.chunks.len() > 1, "expected multiple spill chunks");
        }

        while let IOResult::IO(_) = ht.load_spilled_partition(partition_idx, None).unwrap() {}
        assert!(ht.is_partition_loaded(partition_idx));

        let entry = ht
            .probe_partition(partition_idx, &key, None)
            .unwrap()
            .unwrap();
        assert_eq!(entry.rowid, 0);

        let mut matches = 1usize;
        while ht.next_match().unwrap().is_some() {
            matches += 1;
        }
        assert_eq!(matches, 2048);
    }

    #[test]
    fn test_load_partition_empty_chunk() {
        let io: Arc<dyn IO> = Arc::new(MemoryIO::new());
        let config = HashTableConfig {
            initial_buckets: 4,
            mem_budget: 1024,
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Default,
            track_matched: false,
            partition_count: Some(16),
        };
        let mut ht = HashTable::new(config, io.clone()).unwrap();
        let partitioning = Partitioning::new(16);
        let temp_file = TempFile::with_temp_store(&io, crate::TempStore::Default).unwrap();

        let mut partition = SpilledPartition::new(0);
        partition.add_chunk(0, 0, 0).unwrap();

        let spill_state = SpillState {
            partition_buffers: (0..partitioning.count)
                .map(|_| PartitionBuffer::new())
                .try_collect()
                .unwrap(),
            partitions: vec![partition],
            next_spill_offset: 0,
            temp_file,
            partitioning,
        };
        ht.spill_state = Some(spill_state);
        ht.state = HashTableState::Probing;

        while let IOResult::IO(_) = ht.load_spilled_partition(0, None).unwrap() {}
        assert!(ht.is_partition_loaded(0));
    }

    #[test]
    fn test_load_partition_truncated_chunk() {
        let io: Arc<dyn IO> = Arc::new(MemoryIO::new());
        let config = HashTableConfig {
            initial_buckets: 4,
            mem_budget: 1024,
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Default,
            track_matched: false,
            partition_count: Some(16),
        };
        let mut ht = HashTable::new(config, io.clone()).unwrap();

        let entry = HashEntry::new(1, vec![Value::from_i64(1)], 7);
        let mut buf = vec![];
        entry.serialize(&mut buf).unwrap();
        let truncated = &buf[..buf.len() - 1];

        let temp_file = TempFile::with_temp_store(&io, crate::TempStore::Default).unwrap();
        let write_buf = Buffer::new_temporary(truncated.len());
        write_buf.as_mut_slice().copy_from_slice(truncated);
        let write_buf = Arc::new(write_buf);
        let completion = temp_file
            .file
            .pwrite(0, write_buf, Completion::new_write(|_| {}))
            .unwrap();
        assert!(completion.finished(), "memory write should complete");

        let partitioning = Partitioning::new(16);
        let mut partition = SpilledPartition::new(0);
        partition.add_chunk(0, truncated.len(), 1).unwrap();

        let spill_state = SpillState {
            partition_buffers: (0..partitioning.count)
                .map(|_| PartitionBuffer::new())
                .try_collect()
                .unwrap(),
            partitions: vec![partition],
            next_spill_offset: truncated.len() as u64,
            temp_file,
            partitioning,
        };
        ht.spill_state = Some(spill_state);
        ht.state = HashTableState::Probing;

        let mut saw_err = false;
        loop {
            match ht.load_spilled_partition(0, None) {
                Ok(IOResult::Done(())) => break,
                Ok(IOResult::IO(_)) => continue,
                Err(_) => {
                    saw_err = true;
                    break;
                }
            }
        }

        assert!(saw_err, "truncated chunk should return an error");
    }

    #[test]
    fn test_hash_function_respects_collation_nocase() {
        use crate::types::{TextRef, TextSubtype};

        let keys1 = vec![ValueRef::Text(TextRef::new("Hello", TextSubtype::Text))];
        let keys2 = vec![ValueRef::Text(TextRef::new("hello", TextSubtype::Text))];

        // Under BINARY: hashes must differ
        let bin_coll = vec![CollationSeq::Binary];
        let h1_bin = hash_join_key(&keys1, &bin_coll);
        let h2_bin = hash_join_key(&keys2, &bin_coll);
        assert_ne!(h1_bin, h2_bin);

        // Under NOCASE: hashes should be equal
        let nocase_coll = vec![CollationSeq::NoCase];
        let h1_nc = hash_join_key(&keys1, &nocase_coll);
        let h2_nc = hash_join_key(&keys2, &nocase_coll);
        assert_eq!(h1_nc, h2_nc);
    }

    #[test]
    fn test_hash_nocase_preserves_non_ascii() {
        use crate::types::{TextRef, TextSubtype};

        // SQLite NOCASE only affects ASCII a-z/A-Z.
        // Non-ASCII characters like ü should hash identically regardless of case conversion.
        let keys1 = vec![ValueRef::Text(TextRef::new("über", TextSubtype::Text))];
        let keys2 = vec![ValueRef::Text(TextRef::new("ÜBER", TextSubtype::Text))];

        // Under NOCASE: ASCII portion differs (b/B), so hashes should differ
        // (because SQLite NOCASE doesn't handle Unicode case folding)
        let nocase_coll = vec![CollationSeq::NoCase];
        let h1 = hash_join_key(&keys1, &nocase_coll);
        let h2 = hash_join_key(&keys2, &nocase_coll);

        // The 'b' and 'B' will be lowercased to 'b', but the 'ü' and 'Ü' are not
        // ASCII so they remain as-is. Since ü != Ü at byte level, hashes will differ.
        // This is correct SQLite NOCASE behavior (ASCII-only case folding).
        assert_ne!(
            h1, h2,
            "non-ASCII chars should not be case-folded by NOCASE"
        );
    }

    #[test]
    fn test_hash_nocase_embedded_nul_matches_equality() {
        use crate::types::{TextRef, TextSubtype};

        let nocase_coll = vec![CollationSeq::NoCase];
        let keys1 = vec![ValueRef::Text(TextRef::new("A\0x", TextSubtype::Text))];
        let keys2 = vec![ValueRef::Text(TextRef::new("a\0y", TextSubtype::Text))];
        let keys3 = vec![ValueRef::Text(TextRef::new("a\0yz", TextSubtype::Text))];

        assert!(values_equal(keys1[0], keys2[0], CollationSeq::NoCase));
        assert_eq!(
            hash_join_key(&keys1, &nocase_coll),
            hash_join_key(&keys2, &nocase_coll)
        );

        assert!(!values_equal(keys1[0], keys3[0], CollationSeq::NoCase));
        assert_ne!(
            hash_join_key(&keys1, &nocase_coll),
            hash_join_key(&keys3, &nocase_coll)
        );
    }

    #[test]
    fn test_values_equal_with_collations() {
        use crate::types::{TextRef, TextSubtype};

        let h1 = ValueRef::Text(TextRef::new("Hello  ", TextSubtype::Text));
        let h2 = ValueRef::Text(TextRef::new("hello", TextSubtype::Text));

        // Binary: case / trailing spaces matter
        assert!(!values_equal(h1, h2, CollationSeq::Binary));

        // NOCASE: case-insensitive but trailing spaces still matter -> likely false
        assert!(!values_equal(h1, h2, CollationSeq::NoCase));

        // RTRIM: ignore trailing spaces, but case is still significant
        let h3 = ValueRef::Text(TextRef::new("Hello", TextSubtype::Text));
        assert!(values_equal(h1, h3, CollationSeq::Rtrim));
    }

    #[test]
    fn test_keys_equal_with_collations() {
        use crate::types::{TextRef, TextSubtype};

        let key1 = vec![Value::Text("Hello".into())];
        let key2 = vec![ValueRef::Text(TextRef::new("hello", TextSubtype::Text))];

        // Binary: not equal
        assert!(!keys_equal(&key1, &key2, &[CollationSeq::Binary]));

        // NOCASE: equal
        assert!(keys_equal(&key1, &key2, &[CollationSeq::NoCase]));
    }

    #[test]
    fn test_hash_entry_deserialization_truncated() {
        let entry = HashEntry::new(123, vec![Value::from_i64(1), Value::Text("abc".into())], 42);

        let mut buf = vec![];
        entry.serialize(&mut buf).unwrap();

        // Cut off the buffer mid-entry
        let truncated = &buf[..buf.len() - 2];

        let res = HashEntry::deserialize(truncated);
        assert!(
            res.is_err(),
            "truncated buffer should be rejected as corrupt"
        );
    }

    #[test]
    fn test_hash_entry_deserialization_garbage_type_tag() {
        let entry = HashEntry::new(1, vec![Value::from_i64(10)], 7);
        let mut buf = vec![];
        entry.serialize(&mut buf).unwrap();

        // Compute the exact offset of the *first* type tag.
        // Layout: [0..8] hash | [8..16] rowid | varint(num_keys) | type | payload...
        let mut corrupted = buf.clone();

        let mut offset = 16;
        let (_num_keys, varint_len) = read_varint(&corrupted[offset..]).unwrap();
        offset += varint_len;
        corrupted[offset] = 0xFF;

        let res = HashEntry::deserialize(&corrupted);
        assert!(
            res.is_err(),
            "invalid type tag should be rejected as corrupt"
        );
    }

    fn insert_many_force_spill(ht: &mut HashTable, start: i64, count: i64) {
        for i in 0..count {
            let rowid = start + i;
            let key = vec![Value::from_i64(rowid)];
            let _ = ht.insert(key, rowid, vec![], None);
        }
    }

    #[test]
    fn test_hash_table_spill_and_load_partition_round_trip() {
        let io = Arc::new(MemoryIO::new());
        let config = HashTableConfig {
            initial_buckets: 4,
            // very small budget to force spill
            mem_budget: 1024,
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Default,
            track_matched: false,
            ..Default::default()
        };
        let mut ht = HashTable::new(config, io).unwrap();

        // Insert enough fat rows to exceed budget and force spills
        insert_many_force_spill(&mut ht, 0, 1024);

        let _ = ht.finalize_build(None).unwrap();
        assert!(ht.has_spilled(), "hash table should have spilled");

        // Pick a key and find its partition
        let probe_key = vec![Value::from_i64(10)];
        let partition_idx = ht.partition_for_keys(&probe_key).unwrap();

        // Load that partition into memory
        match ht.load_spilled_partition(partition_idx, None).unwrap() {
            IOResult::Done(()) => {}
            IOResult::IO(_) => panic!("test harness must drive IO completions here"),
        }

        assert!(
            ht.is_partition_loaded(partition_idx),
            "partition must be resident after load_spilled_partition"
        );

        // Probe via partition API
        let entry = ht.probe_partition(partition_idx, &probe_key, None).unwrap();
        assert!(entry.is_some()); // here
        assert_eq!(entry.unwrap().rowid, 10);
    }

    #[test]
    fn test_partition_lru_eviction() {
        let io = Arc::new(MemoryIO::new());
        // tiny mem_budget so only ~1 partition can stay resident
        let config = HashTableConfig {
            initial_buckets: 4,
            mem_budget: 8 * 1024,
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Default,
            track_matched: false,
            partition_count: None,
        };
        let mut ht = HashTable::new(config, io).unwrap();

        // Insert two disjoint key ranges that will hash to different partitions
        insert_many_force_spill(&mut ht, 0, 256);
        insert_many_force_spill(&mut ht, 256, 1024);

        let _ = ht.finalize_build(None).unwrap();
        assert!(ht.has_spilled());

        let key_a = vec![Value::from_i64(1)];
        let key_b = vec![Value::from_i64(10_001)];
        let pa = ht.partition_for_keys(&key_a).unwrap();
        let pb = ht.partition_for_keys(&key_b).unwrap();
        assert_ne!(pa, pb);

        // Load partition A
        while let IOResult::IO(_) = ht.load_spilled_partition(pa, None).unwrap() {}
        assert!(ht.is_partition_loaded(pa));

        // Now load partition B, this should (under tight memory) evict A
        let _ = ht.load_spilled_partition(pb, None).unwrap();
        assert!(ht.is_partition_loaded(pb));

        // Depending on mem_budget and actual entry sizes, A should now be evicted
        // We can't *guarantee* that without knowing exact sizes, but in practice
        // this test will detect regressions in the LRU bookkeeping.
        assert!(
            !ht.is_partition_loaded(pa) || ht.loaded_partitions_mem <= ht.mem_budget,
            "either partition A is evicted, or loaded memory is within budget"
        );
    }

    #[test]
    fn test_probe_partition_with_duplicate_keys() {
        let io = Arc::new(MemoryIO::new());
        let config = HashTableConfig {
            initial_buckets: 4,
            mem_budget: 8 * 1024,
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Default,
            track_matched: false,
            partition_count: None,
        };
        let mut ht = HashTable::new(config, io).unwrap();

        let key = vec![Value::from_i64(42)];
        for i in 0..1024 {
            match ht.insert(key.clone(), 1000 + i, vec![], None).unwrap() {
                IOResult::Done(()) => {}
                IOResult::IO(_) => panic!("memory IO"),
            }
        }
        match ht.finalize_build(None).unwrap() {
            IOResult::Done(()) => {}
            IOResult::IO(_) => panic!("memory IO"),
        }

        assert!(ht.has_spilled());
        let partition_idx = ht.partition_for_keys(&key).unwrap();

        match ht.load_spilled_partition(partition_idx, None).unwrap() {
            IOResult::Done(()) => {}
            IOResult::IO(_) => panic!("memory IO"),
        }
        assert!(ht.is_partition_loaded(partition_idx));

        // First probe should give us the first rowid
        let entry1 = ht
            .probe_partition(partition_idx, &key, None)
            .unwrap()
            .unwrap();
        assert_eq!(entry1.rowid, 1000);

        // Then iterate through the rest with next_match
        for i in 0..1023 {
            let next = ht.next_match().unwrap().unwrap();
            assert_eq!(next.rowid, 1001 + i);
        }
        assert!(ht.next_match().unwrap().is_none());
    }

    #[test]
    fn test_hash_table_with_payload() {
        let io = Arc::new(MemoryIO::new());
        let config = HashTableConfig {
            initial_buckets: 4,
            mem_budget: 1024 * 1024,
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Default,
            track_matched: false,
            partition_count: None,
        };
        let mut ht = HashTable::new(config, io).unwrap();

        // Insert entries with payload values (simulating cached result columns)
        let key1 = vec![Value::from_i64(1)];
        let payload1 = vec![
            Value::Text("Alice".into()),
            Value::from_i64(30),
            Value::from_f64(1000.50),
        ];
        let _ = ht.insert(key1.clone(), 100, payload1, None).unwrap();

        let key2 = vec![Value::from_i64(2)];
        let payload2 = vec![
            Value::Text("Bob".into()),
            Value::from_i64(25),
            Value::from_f64(2000.75),
        ];
        let _ = ht.insert(key2.clone(), 200, payload2, None).unwrap();

        let _ = ht.finalize_build(None);

        // Probe and verify payload is returned correctly
        let result = ht.probe(key1, None).unwrap();
        assert!(result.is_some());
        let entry1 = result.unwrap();
        assert_eq!(entry1.rowid, 100);
        assert!(entry1.has_payload());
        assert_eq!(entry1.payload_values.len(), 3);
        assert_eq!(entry1.payload_values[0], Value::Text("Alice".into()));
        assert_eq!(entry1.payload_values[1], Value::from_i64(30));
        assert_eq!(entry1.payload_values[2], Value::from_f64(1000.50));

        let result = ht.probe(key2, None).unwrap();
        assert!(result.is_some());
        let entry2 = result.unwrap();
        assert_eq!(entry2.rowid, 200);
        assert!(entry2.has_payload());
        assert_eq!(entry2.payload_values[0], Value::Text("Bob".into()));
        assert_eq!(entry2.payload_values[1], Value::from_i64(25));
        assert_eq!(entry2.payload_values[2], Value::from_f64(2000.75));
    }

    #[test]
    fn test_hash_table_payload_with_nulls() {
        let io = Arc::new(MemoryIO::new());
        let config = HashTableConfig {
            initial_buckets: 4,
            mem_budget: 1024 * 1024,
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Default,
            track_matched: false,
            partition_count: None,
        };
        let mut ht = HashTable::new(config, io).unwrap();

        // Insert entry with NULL values in payload
        let key = vec![Value::from_i64(1)];
        let payload = vec![Value::Null, Value::Text("test".into()), Value::Null];
        let _ = ht.insert(key.clone(), 100, payload, None).unwrap();

        let _ = ht.finalize_build(None);

        let result = ht.probe(key, None).unwrap();
        assert!(result.is_some());
        let entry = result.unwrap();
        assert_eq!(entry.payload_values.len(), 3);
        assert_eq!(entry.payload_values[0], Value::Null);
        assert_eq!(entry.payload_values[1], Value::Text("test".into()));
        assert_eq!(entry.payload_values[2], Value::Null);
    }

    #[test]
    fn test_null_keys_are_skipped() {
        // In SQL, NULL = NULL is false (actually NULL which is falsy).
        // Hash joins should skip rows with NULL keys during both insert and probe.
        let io = Arc::new(MemoryIO::new());
        let config = HashTableConfig {
            initial_buckets: 4,
            mem_budget: 1024 * 1024,
            num_keys: 2,
            collations: vec![CollationSeq::Binary, CollationSeq::Binary],
            temp_store: crate::TempStore::Default,
            track_matched: false,
            partition_count: None,
        };
        let mut ht = HashTable::new(config, io).unwrap();

        // Insert entry with NULL key - should be silently skipped
        let null_key = vec![Value::Null, Value::from_i64(1)];
        let _ = ht.insert(null_key.clone(), 100, vec![], None).unwrap();

        // Insert entry with non-NULL keys
        let valid_key = vec![Value::from_i64(1), Value::from_i64(2)];
        let _ = ht.insert(valid_key.clone(), 200, vec![], None).unwrap();

        // Insert another entry where second key is NULL
        let null_key2 = vec![Value::from_i64(1), Value::Null];
        let _ = ht.insert(null_key2.clone(), 300, vec![], None).unwrap();

        let _ = ht.finalize_build(None);

        // Only one entry should be in the table (the one with valid keys)
        assert_eq!(ht.num_entries, 1);

        // Probing with NULL key should return None
        let result = ht.probe(null_key, None).unwrap();
        assert!(result.is_none());

        // Probing with valid key should return the entry
        let result = ht.probe(valid_key, None).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().rowid, 200);

        // Probing with NULL in second position should also return None
        let result = ht.probe(null_key2, None).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_hash_table_payload_with_blobs() {
        let io = Arc::new(MemoryIO::new());
        let config = HashTableConfig {
            initial_buckets: 4,
            mem_budget: 1024 * 1024,
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Default,
            track_matched: false,
            partition_count: None,
        };
        let mut ht = HashTable::new(config, io).unwrap();

        // Insert entry with blob payload
        let key = vec![Value::from_i64(1)];
        let blob_data = std::vec![0xDE, 0xAD, 0xBE, 0xEF];
        let payload = vec![
            Value::from_slice(&blob_data).expect(crate::alloc::ALLOC_ERR_MSG),
            Value::from_i64(42),
        ];
        let _ = ht.insert(key.clone(), 100, payload, None).unwrap();

        let _ = ht.finalize_build(None);

        let result = ht.probe(key, None).unwrap();
        assert!(result.is_some());
        let entry = result.unwrap();
        assert_eq!(entry.payload_values.len(), 2);
        assert_eq!(
            entry.payload_values[0],
            Value::from_slice(&blob_data).expect(crate::alloc::ALLOC_ERR_MSG)
        );
        assert_eq!(entry.payload_values[1], Value::from_i64(42));
    }

    #[test]
    fn test_hash_table_payload_duplicate_keys() {
        let io = Arc::new(MemoryIO::new());
        let config = HashTableConfig {
            initial_buckets: 4,
            mem_budget: 1024 * 1024,
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Default,
            track_matched: false,
            partition_count: None,
        };
        let mut ht = HashTable::new(config, io).unwrap();

        // Insert multiple entries with the same key but different payloads
        let key = vec![Value::from_i64(42)];
        let _ = ht
            .insert(
                key.clone(),
                100,
                vec![Value::Text("first".into()), Value::from_i64(1)],
                None,
            )
            .unwrap();
        let _ = ht
            .insert(
                key.clone(),
                200,
                vec![Value::Text("second".into()), Value::from_i64(2)],
                None,
            )
            .unwrap();
        let _ = ht
            .insert(
                key.clone(),
                300,
                vec![Value::Text("third".into()), Value::from_i64(3)],
                None,
            )
            .unwrap();

        let _ = ht.finalize_build(None);

        // First probe should return first match
        let result = ht.probe(key, None).unwrap();
        assert!(result.is_some());
        let entry1 = result.unwrap();
        assert_eq!(entry1.rowid, 100);
        assert_eq!(entry1.payload_values[0], Value::Text("first".into()));
        assert_eq!(entry1.payload_values[1], Value::from_i64(1));

        // next_match should return subsequent matches with their payloads
        let entry2 = ht.next_match().unwrap().unwrap();
        assert_eq!(entry2.rowid, 200);
        assert_eq!(entry2.payload_values[0], Value::Text("second".into()));
        assert_eq!(entry2.payload_values[1], Value::from_i64(2));

        let entry3 = ht.next_match().unwrap().unwrap();
        assert_eq!(entry3.rowid, 300);
        assert_eq!(entry3.payload_values[0], Value::Text("third".into()));
        assert_eq!(entry3.payload_values[1], Value::from_i64(3));

        // No more matches
        assert!(ht.next_match().unwrap().is_none());
    }

    #[test]
    fn test_hash_entry_payload_serialization() {
        // Test that payload values survive serialization/deserialization
        let entry = HashEntry::new_with_payload(
            12345,
            vec![Value::from_i64(1), Value::Text("key".into())],
            100,
            vec![
                Value::Text("payload_text".into()),
                Value::from_i64(999),
                Value::from_f64(std::f64::consts::PI),
                Value::Null,
                Value::from_slice(&[1, 2, 3, 4]).expect(crate::alloc::ALLOC_ERR_MSG),
            ],
        );

        let mut buf = vec![];
        entry.serialize(&mut buf).unwrap();

        let (deserialized, bytes_consumed) = HashEntry::deserialize(&buf).unwrap();
        assert_eq!(bytes_consumed, buf.len());

        // Verify key values
        assert_eq!(deserialized.hash, entry.hash);
        assert_eq!(deserialized.rowid, entry.rowid);
        assert_eq!(deserialized.key_values.len(), 2);
        assert_eq!(deserialized.key_values[0], Value::from_i64(1));
        assert_eq!(deserialized.key_values[1], Value::Text("key".into()));

        // Verify payload values
        assert_eq!(deserialized.payload_values.len(), 5);
        assert_eq!(
            deserialized.payload_values[0],
            Value::Text("payload_text".into())
        );
        assert_eq!(deserialized.payload_values[1], Value::from_i64(999));
        assert_eq!(
            deserialized.payload_values[2],
            Value::from_f64(std::f64::consts::PI)
        );
        assert_eq!(deserialized.payload_values[3], Value::Null);
        assert_eq!(
            deserialized.payload_values[4],
            Value::from_slice(&[1, 2, 3, 4]).expect(crate::alloc::ALLOC_ERR_MSG)
        );
    }

    #[test]
    fn test_hash_entry_empty_payload() {
        // Test that entries without payload work correctly
        let entry = HashEntry::new(12345, vec![Value::from_i64(1)], 100);

        assert!(!entry.has_payload());
        assert!(entry.payload_values.is_empty());

        // Serialization should still work
        let mut buf = vec![];
        entry.serialize(&mut buf).unwrap();

        let (deserialized, _) = HashEntry::deserialize(&buf).unwrap();
        assert!(!deserialized.has_payload());
        assert!(deserialized.payload_values.is_empty());
        assert_eq!(deserialized.rowid, 100);
    }

    #[test]
    fn test_hash_entry_size_includes_payload() {
        let entry_no_payload = HashEntry::new(12345, vec![Value::from_i64(1)], 100);

        let entry_with_payload = HashEntry::new_with_payload(
            12345,
            vec![Value::from_i64(1)],
            100,
            vec![
                Value::Text("a]long payload string".into()),
                Value::from_i64(42),
            ],
        );

        // Entry with payload should have larger size
        assert!(entry_with_payload.size_bytes() > entry_no_payload.size_bytes());
    }

    // ── Grace hash join tests ──────────────────────────────────────

    /// Helper: build a spilled hash table with given keys and payloads
    fn make_spilled_ht_with_payload(
        io: Arc<dyn IO>,
        build_keys: &[(i64, Vec<Value>)], // (rowid, key_values)
        payload: bool,
    ) -> HashTable {
        let config = HashTableConfig {
            initial_buckets: 4,
            mem_budget: 1024, // tiny, forces spill
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Default,
            track_matched: false,
            ..Default::default()
        };
        let mut ht = HashTable::new(config, io).unwrap();
        for (rowid, keys) in build_keys {
            let payload_values = if payload {
                vec![Value::Text(format!("payload_{rowid}").into())]
            } else {
                vec![]
            };
            let _ = ht.insert(keys.clone(), *rowid, payload_values, None);
        }
        let _ = ht.finalize_build(None).unwrap();
        ht
    }

    /// Helper: run grace processing with the new fine-grained API.
    /// Returns (build_rowid, probe_rowid) pairs for all matches found.
    fn run_grace_processing(ht: &mut HashTable) -> Vec<(i64, i64)> {
        let _ = ht.finalize_probe_spill(None).unwrap();
        let mut matches = vec![];

        if !ht.grace_begin().unwrap() {
            return matches;
        }

        // Partition loop
        loop {
            match ht.grace_load_current_partition(None).unwrap() {
                IOResult::Done(true) => {}
                IOResult::Done(false) => break,
                _ => panic!("unexpected IO"),
            }

            // Probe entry loop
            loop {
                let entry = match ht.grace_next_probe_entry().unwrap() {
                    IOResult::Done(entry) => entry,
                    IOResult::IO(_) => panic!("unexpected IO"),
                };
                let Some(entry) = entry else {
                    break;
                };

                // Use probe_partition + next_match to find build matches
                let key_values = entry.key_values;
                let probe_rowid = entry.probe_rowid;
                let partition_idx = ht.partition_for_keys(&key_values).unwrap();

                if ht
                    .probe_partition(partition_idx, &key_values, None)
                    .unwrap()
                    .is_some()
                {
                    // First match from probe_partition
                    let build_entry = ht
                        .probe_partition(partition_idx, &key_values, None)
                        .unwrap();
                    // Re-probe to get entry again
                    if let Some(build_entry) = build_entry {
                        matches.push((build_entry.rowid, probe_rowid));
                    }
                    // Get additional matches via next_match
                    while let Some(build_entry) = ht.next_match().unwrap() {
                        matches.push((build_entry.rowid, probe_rowid));
                    }
                }
            }

            if !ht.grace_advance_partition() {
                break;
            }
        }
        matches
    }

    #[test]
    fn test_grace_basic() {
        let io = Arc::new(MemoryIO::new());
        let build_keys: Vec<(i64, Vec<Value>)> = (0..200)
            .map(|i| (i, vec![Value::from_i64(i)]))
            .try_collect()
            .unwrap();
        let mut ht = make_spilled_ht_with_payload(io, &build_keys, true);
        assert!(ht.has_spilled(), "should have spilled");

        // Buffer probe rows for keys that map to spilled partitions
        let mut buffered = 0;
        for i in 0..200 {
            let key = vec![Value::from_i64(i)];
            let partition_idx = ht.partition_for_keys(&key).unwrap();
            if !ht.is_partition_loaded(partition_idx) {
                let _ = ht.buffer_probe_row(key, i + 1000, None).unwrap();
                buffered += 1;
            }
        }
        assert!(buffered > 0, "should have buffered some probe rows");

        let matches = run_grace_processing(&mut ht);

        // Every buffered probe row should have found a match
        assert_eq!(
            matches.len(),
            buffered,
            "each buffered probe row should match exactly one build row"
        );
        // Verify correctness: build_rowid should equal probe_rowid - 1000
        for (build_rowid, probe_rowid) in &matches {
            assert_eq!(
                *build_rowid,
                probe_rowid - 1000,
                "build_rowid should match probe key"
            );
        }
    }

    #[test]
    fn test_grace_no_spill_noop() {
        let io = Arc::new(MemoryIO::new());
        // Use large budget so nothing spills
        let config = HashTableConfig {
            initial_buckets: 4,
            mem_budget: 1024 * 1024,
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Default,
            track_matched: false,
            ..Default::default()
        };
        let mut ht = HashTable::new(config, io).unwrap();
        for i in 0..10 {
            let _ = ht.insert(vec![Value::from_i64(i)], i, vec![], None);
        }
        let _ = ht.finalize_build(None).unwrap();
        assert!(!ht.has_spilled(), "should NOT have spilled");

        // grace_begin should return false since nothing was spilled
        assert!(
            !ht.grace_begin().unwrap(),
            "grace_begin should return false when nothing spilled"
        );
    }

    #[test]
    fn test_grace_duplicate_keys() {
        let io = Arc::new(MemoryIO::new());
        // Insert multiple build rows with same key
        let mut build_keys: Vec<(i64, Vec<Value>)> = vec![];
        for i in 0..100 {
            // 3 build rows per key value
            build_keys.push((i * 3, vec![Value::from_i64(i)]));
            build_keys.push((i * 3 + 1, vec![Value::from_i64(i)]));
            build_keys.push((i * 3 + 2, vec![Value::from_i64(i)]));
        }
        let mut ht = make_spilled_ht_with_payload(io, &build_keys, false);
        assert!(ht.has_spilled());

        // Buffer probe rows
        let mut buffered_keys = vec![];
        for i in 0..100 {
            let key = vec![Value::from_i64(i)];
            let partition_idx = ht.partition_for_keys(&key).unwrap();
            if !ht.is_partition_loaded(partition_idx) {
                let _ = ht.buffer_probe_row(key, i + 500, None).unwrap();
                buffered_keys.push(i);
            }
        }

        let matches = run_grace_processing(&mut ht);

        // Each buffered probe key should match 3 build rows
        assert_eq!(
            matches.len(),
            buffered_keys.len() * 3,
            "each probe key should find 3 matches"
        );
    }

    #[test]
    fn test_grace_empty_partitions() {
        let io = Arc::new(MemoryIO::new());
        // Build with keys 0..100, probe with keys 200..300 (no overlap)
        let build_keys: Vec<(i64, Vec<Value>)> = (0..200)
            .map(|i| (i, vec![Value::from_i64(i)]))
            .try_collect()
            .unwrap();
        let mut ht = make_spilled_ht_with_payload(io, &build_keys, false);
        assert!(ht.has_spilled());

        // Buffer probe rows with non-matching keys
        for i in 1000..1050 {
            let key = vec![Value::from_i64(i)];
            let partition_idx = ht.partition_for_keys(&key).unwrap();
            if !ht.is_partition_loaded(partition_idx) {
                let _ = ht.buffer_probe_row(key, i, None).unwrap();
            }
        }

        let matches = run_grace_processing(&mut ht);
        assert_eq!(
            matches.len(),
            0,
            "non-matching keys should produce no matches"
        );
    }

    #[test]
    fn test_grace_null_keys() {
        let io = Arc::new(MemoryIO::new());
        let build_keys: Vec<(i64, Vec<Value>)> = (0..200)
            .map(|i| (i, vec![Value::from_i64(i)]))
            .try_collect()
            .unwrap();
        let mut ht = make_spilled_ht_with_payload(io, &build_keys, false);
        assert!(ht.has_spilled());

        // Buffer a probe row with NULL key - should be skipped
        let null_key = vec![Value::Null];
        // NULL keys can't match, so we just verify no crash
        let partition_idx = ht.partition_for_keys(&[Value::from_i64(0)]).unwrap();
        if !ht.is_partition_loaded(partition_idx) {
            // Buffer with a valid key to ensure grace processing runs
            let _ = ht
                .buffer_probe_row(vec![Value::from_i64(0)], 999, None)
                .unwrap();
        }
        // Buffer a null key row to the same partition
        let _ = ht.buffer_probe_row(null_key, 888, None).unwrap();

        let _ = ht.finalize_probe_spill(None).unwrap();
        // Should not crash; NULL key entries should return from grace_next_probe_entry
        assert!(
            ht.grace_begin().unwrap(),
            "should have partitions to process"
        );
        loop {
            match ht.grace_load_current_partition(None).unwrap() {
                IOResult::Done(true) => {}
                IOResult::Done(false) => break,
                _ => panic!("unexpected IO"),
            }
            loop {
                let entry = match ht.grace_next_probe_entry().unwrap() {
                    IOResult::Done(entry) => entry,
                    IOResult::IO(_) => panic!("unexpected IO"),
                };
                if entry.is_none() {
                    break;
                }
                // NULL key probe entries are still returned; the VDBE's HashProbe
                // handles NULL skip. Just verify no crash.
            }
            if !ht.grace_advance_partition() {
                break;
            }
        }
    }

    #[test]
    fn test_grace_with_payload() {
        let io = Arc::new(MemoryIO::new());
        let build_keys: Vec<(i64, Vec<Value>)> = (0..200)
            .map(|i| (i, vec![Value::from_i64(i)]))
            .try_collect()
            .unwrap();
        let mut ht = make_spilled_ht_with_payload(io, &build_keys, true);
        assert!(ht.has_spilled());

        // Buffer some probe rows
        let mut buffered = 0;
        for i in 0..200 {
            let key = vec![Value::from_i64(i)];
            let partition_idx = ht.partition_for_keys(&key).unwrap();
            if !ht.is_partition_loaded(partition_idx) {
                let _ = ht.buffer_probe_row(key, i + 1000, None).unwrap();
                buffered += 1;
            }
        }

        let _ = ht.finalize_probe_spill(None).unwrap();
        assert!(
            ht.grace_begin().unwrap(),
            "should have partitions to process"
        );

        let mut match_count = 0;
        loop {
            match ht.grace_load_current_partition(None).unwrap() {
                IOResult::Done(true) => {}
                IOResult::Done(false) => break,
                _ => panic!("unexpected IO"),
            }
            loop {
                let entry = match ht.grace_next_probe_entry().unwrap() {
                    IOResult::Done(entry) => entry,
                    IOResult::IO(_) => panic!("unexpected IO"),
                };
                let Some(entry) = entry else {
                    break;
                };
                let key_values = entry.key_values;
                let partition_idx = ht.partition_for_keys(&key_values).unwrap();
                if let Some(build_entry) = ht
                    .probe_partition(partition_idx, &key_values, None)
                    .unwrap()
                {
                    // Check payload was correctly round-tripped
                    let expected_payload = format!("payload_{}", build_entry.rowid);
                    assert_eq!(build_entry.payload_values.len(), 1);
                    match &build_entry.payload_values[0] {
                        Value::Text(t) => assert_eq!(t.as_str(), expected_payload.as_str()),
                        other => panic!("expected text payload, got {other:?}"),
                    }
                    match_count += 1;
                }
            }
            if !ht.grace_advance_partition() {
                break;
            }
        }
        assert_eq!(match_count, buffered);
    }

    #[test]
    fn test_grace_unmatched_scan_uses_current_partition() {
        let io = Arc::new(MemoryIO::new());
        let config = HashTableConfig {
            initial_buckets: 4,
            mem_budget: 4096,
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Default,
            track_matched: true,
            partition_count: Some(4),
        };
        let mut ht = HashTable::new(config, io).unwrap();

        for i in 0..400 {
            let key = vec![Value::from_i64(i)];
            let _ = ht.insert(key, i, vec![], None);
        }
        let _ = ht.finalize_build(None).unwrap();
        assert!(ht.has_spilled(), "should have spilled");

        let mut keys_by_partition = std::collections::BTreeMap::<usize, Vec<i64>>::new();
        for i in 0..400 {
            let partition_idx = ht.partition_for_keys(&[Value::from_i64(i)]).unwrap();
            keys_by_partition
                .entry(partition_idx)
                .or_insert_with(|| vec![])
                .push(i);
        }

        let partitions_to_process: Vec<usize> = ht
            .spill_state
            .as_ref()
            .expect("spill state")
            .partitions
            .iter()
            .filter(|partition| !partition.chunks.is_empty())
            .map(|partition| partition.partition_idx)
            .try_collect()
            .unwrap();
        assert!(
            partitions_to_process.len() >= 2,
            "test requires at least two spilled partitions"
        );

        let first_partition = partitions_to_process[0];
        let second_partition = partitions_to_process[1];
        let first_keys = keys_by_partition
            .get(&first_partition)
            .cloned()
            .expect("first partition keys");
        let second_keys = keys_by_partition
            .get(&second_partition)
            .cloned()
            .expect("second partition keys");

        for key in &first_keys {
            let _ = ht
                .buffer_probe_row(vec![Value::from_i64(*key)], key + 10_000, None)
                .unwrap();
        }
        let _ = ht.finalize_probe_spill(None).unwrap();
        assert!(ht.grace_begin().unwrap(), "should enter grace");

        match ht.grace_load_current_partition(None).unwrap() {
            IOResult::Done(true) => {}
            other => panic!("unexpected grace load result: {other:?}"),
        }
        assert_eq!(
            ht.grace_state
                .as_ref()
                .expect("grace state")
                .current_partition_idx(),
            Some(first_partition)
        );

        loop {
            let entry = match ht.grace_next_probe_entry().unwrap() {
                IOResult::Done(entry) => entry,
                IOResult::IO(_) => panic!("unexpected IO"),
            };
            let Some(entry) = entry else {
                break;
            };
            let partition_idx = ht.partition_for_keys(&entry.key_values).unwrap();
            if ht
                .probe_partition(partition_idx, &entry.key_values, None)
                .unwrap()
                .is_some()
            {
                ht.mark_current_matched();
                while ht.next_match().unwrap().is_some() {
                    ht.mark_current_matched();
                }
            }
        }

        ht.begin_unmatched_scan();
        assert!(
            ht.next_unmatched().is_none(),
            "all rows in the first partition were matched"
        );

        assert!(
            ht.grace_advance_partition(),
            "should have another partition"
        );
        match ht.grace_load_current_partition(None).unwrap() {
            IOResult::Done(true) => {}
            other => panic!("unexpected grace load result: {other:?}"),
        }
        assert_eq!(
            ht.grace_state
                .as_ref()
                .expect("grace state")
                .current_partition_idx(),
            Some(second_partition)
        );

        match ht.load_spilled_partition(first_partition, None).unwrap() {
            IOResult::Done(()) => {}
            other => panic!("unexpected spill load result: {other:?}"),
        }

        ht.begin_unmatched_scan();
        assert_eq!(
            ht.unmatched_scan_current_partition(),
            Some(second_partition),
            "grace unmatched scan must target the active grace partition"
        );
        match ht.load_spilled_partition(second_partition, None).unwrap() {
            IOResult::Done(()) => {}
            other => panic!("unexpected spill load result: {other:?}"),
        }

        let mut unmatched_rowids = Vec::new();
        while let Some(entry) = ht.next_unmatched() {
            unmatched_rowids.push(entry.rowid);
        }
        unmatched_rowids.sort_unstable();

        let mut expected = second_keys;
        expected.sort_unstable();
        assert_eq!(unmatched_rowids, expected);
    }

    #[test]
    fn test_unmatched_scan_preserves_in_memory_partitions_before_grace() {
        let io = Arc::new(MemoryIO::new());
        let config = HashTableConfig {
            // Enough buckets that the table doesn't grow before it spills (AUDIT 3.1): the grown
            // array's bytes are freed at the spill, and this test needs its first spill to write
            // a partition to disk.
            initial_buckets: 64,
            mem_budget: 1024,
            num_keys: 1,
            collations: vec![CollationSeq::Binary],
            temp_store: crate::TempStore::Default,
            track_matched: true,
            partition_count: Some(16),
        };
        let mut ht = HashTable::new(config, io).unwrap();

        let mut next_rowid = 0i64;
        while !ht.has_spilled() {
            let _ = ht
                .insert(vec![Value::from_i64(next_rowid)], next_rowid, vec![], None)
                .unwrap();
            next_rowid += 1;
        }

        let partition_keys: std::collections::BTreeMap<usize, Vec<i64>> = (0..4096)
            .map(|i| (ht.partition_for_keys(&[Value::from_i64(i)]).unwrap(), i))
            .fold(
                std::collections::BTreeMap::new(),
                |mut acc, (partition, key)| {
                    acc.entry(partition).or_insert_with(|| vec![]).push(key);
                    acc
                },
            );

        let hot_partition = ht
            .spill_state
            .as_ref()
            .expect("spill state")
            .partitions
            .first()
            .map(|partition| partition.partition_idx)
            .expect("expected at least one spilled partition");
        let cold_partition = partition_keys
            .keys()
            .copied()
            .find(|partition_idx| {
                ht.spill_state
                    .as_ref()
                    .expect("spill state")
                    .find_partition(*partition_idx)
                    .is_none()
            })
            .expect("expected at least one partition without spill chunks yet");
        let hot_key = partition_keys
            .get(&hot_partition)
            .and_then(|keys| keys.first())
            .copied()
            .expect("hot partition key");
        let cold_key = partition_keys
            .get(&cold_partition)
            .and_then(|keys| keys.first())
            .copied()
            .expect("cold partition key");

        for _ in 0..160 {
            let _ = ht
                .insert(vec![Value::from_i64(hot_key)], next_rowid, vec![], None)
                .unwrap();
            next_rowid += 1;
        }
        for _ in 0..6 {
            let _ = ht
                .insert(vec![Value::from_i64(cold_key)], next_rowid, vec![], None)
                .unwrap();
            next_rowid += 1;
        }

        let _ = ht.finalize_build(None).unwrap();
        assert!(ht.has_spilled(), "should have spilled");

        let (spilled_partition, mut expected_unmatched) = {
            let spill_state = ht.spill_state.as_ref().expect("spill state");
            let spilled_partition = spill_state
                .partitions
                .iter()
                .find(|partition| !partition.chunks.is_empty())
                .map(|partition| partition.partition_idx)
                .expect("expected at least one spilled partition");
            let expected_unmatched: Vec<i64> = spill_state
                .partitions
                .iter()
                .filter(|partition| partition.chunks.is_empty())
                .flat_map(|partition| {
                    partition
                        .buckets
                        .iter()
                        .flat_map(|bucket| bucket.entries.iter().map(|entry| entry.rowid))
                })
                .try_collect()
                .unwrap();
            (spilled_partition, expected_unmatched)
        };
        assert!(
            !expected_unmatched.is_empty(),
            "expected at least one resident in-memory partition"
        );

        let probe_key = (0..400)
            .map(|i| vec![Value::from_i64(i)])
            .find(|key| ht.partition_for_keys(key).unwrap() == spilled_partition)
            .expect("spilled partition should have at least one key");
        let _ = ht.buffer_probe_row(probe_key, 10_000, None).unwrap();
        assert!(
            ht.has_grace_partitions(),
            "probe buffering should enable grace"
        );

        ht.begin_unmatched_scan();
        let mut actual_unmatched = Vec::new();
        while let Some(entry) = ht.next_unmatched() {
            actual_unmatched.push(entry.rowid);
        }

        expected_unmatched.sort_unstable();
        actual_unmatched.sort_unstable();
        assert_eq!(actual_unmatched, expected_unmatched);
    }
}
