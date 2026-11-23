use fastbloom::BloomFilter as BloomFilterInner;
use std::fmt;
use std::hash::{Hash, Hasher};

use crate::numeric::Numeric;
use crate::types::{Value, ValueRef};
use crate::vdbe::hash_table::normalized_f64_bits;

/// Default number of expected items for bloom filter sizing.
/// This is used when the expected count is not known ahead of time.
const DEFAULT_EXPECTED_ITEMS: u32 = 1024;

/// Default false positive rate (1%).
const DEFAULT_FALSE_POSITIVE_RATE: f32 = 0.01;

/// Amphora (PL.5, `docs/design/planner.md`, "Decision (lane qs5-9)"): how a hash join's filter
/// is sized once its build has ended, from the keys the build measured rather than an estimate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilterSizing {
    /// The false-positive rate per distinct build key: the filter's rate is this times the
    /// build's distinct keys. The planner derives it from the learned hash build and probe
    /// weights and the probe rows it expects to find no match (`bloom_filter_rate_per_key`).
    pub fp_per_key: f64,
    /// The most bytes the filter holds, its key hashes while the build runs included: the hash
    /// join's memory budget. Past it the filter is dropped and every probe row passes.
    pub max_bytes: usize,
}

/// What a filter holds.
enum Store {
    /// A filter of fixed size, filled and probed as rows come (the automatic index's).
    Fixed(BloomFilterInner),
    /// A sized filter's key hashes while its build runs.
    Collecting(Vec<u64>),
    /// A sized filter built from its build's distinct keys.
    Sized(BloomFilterInner),
    /// A sized filter whose build held no key: nothing matches.
    NoKeys,
    /// A sized filter not worth its bits, or past its memory bound: every key passes.
    PassAll,
}

/// A bloom filter for fast probabilistic set membership testing.
///
/// Each bloom filter is associated with a cursor or operation that builds it
/// during ephemeral index/hash table construction.
pub struct BloomFilter {
    store: Store,
    sizing: Option<FilterSizing>,
    /// Number of items inserted into the filter
    count: usize,
    /// Amphora (QT.5, lane qs5-20): the most bytes this filter held at once: a growth of its key
    /// hashes, which may hold the old list and the new while it moves them, and its sealing.
    peak: usize,
}

impl fmt::Debug for BloomFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BloomFilter")
            .field("count", &self.count)
            .field("sizing", &self.sizing)
            .finish_non_exhaustive()
    }
}

fn hash_of_values<'a>(values: impl Iterator<Item = &'a Value>) -> u64 {
    let mut hasher = rapidhash::fast::RapidHasher::default();
    for value in values {
        hash_value(&mut hasher, &value.as_ref());
    }
    hasher.finish()
}

fn hash_of(value: &impl Hash) -> u64 {
    let mut hasher = rapidhash::fast::RapidHasher::default();
    value.hash(&mut hasher);
    hasher.finish()
}

/// `bytes` in whole 64-bit words, fastbloom's unit (and the hashes' while a build collects them).
const fn whole_words(bytes: usize) -> usize {
    match bytes.checked_div(size_of::<u64>()) {
        Some(words) => words,
        None => 0,
    }
}

/// Amphora (PL.5): the filter for a build's distinct key hashes. Its rate is `fp_per_key` times
/// the keys, but never below the rate its memory bound allows: with `m` bits, `n` keys and the
/// best number of hashes a Bloom filter's rate is `exp(-(m/n)·ln²2)` (Broder and Mitzenmacher,
/// "Network Applications of Bloom Filters: A Survey", 2004, §2.1). A rate of one or more filters
/// nothing, so the filter passes every key.
///
/// QT.5 (lane qs5-19): the filter is built while `held` bytes, the hashes it is built from, are
/// still held, so its bits are what the bound leaves beside them: the hashes and the finished
/// filter together never pass `max_bytes`. Its bits are counted before they are allocated, in
/// fastbloom's unit, whole 64-bit words, so a filter over the bound is never made.
fn sized_store(hashes: &[u64], sizing: FilterSizing, held: usize) -> Store {
    if hashes.is_empty() {
        return Store::NoKeys;
    }
    let keys = hashes.len() as f64;
    let ln2_squared = std::f64::consts::LN_2 * std::f64::consts::LN_2;
    let room_words = whole_words(sizing.max_bytes.saturating_sub(held));
    let floor = (-(room_words as f64 * 64.0 / keys) * ln2_squared).exp();
    let rate = (sizing.fp_per_key * keys).max(floor);
    if rate.is_nan() || rate <= 0.0 || rate >= 1.0 {
        return Store::PassAll;
    }
    // The bits `rate` takes over the keys (the same survey's `m = -n·ln(rate)/ln²2`), in words,
    // at most the room's: the floor's rate fills the room.
    let wanted_words = (keys * -rate.ln() / ln2_squared / 64.0).ceil();
    let words = if wanted_words.is_finite() && wanted_words < room_words as f64 {
        // Within the room, so within usize.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let wanted = wanted_words as usize;
        wanted.max(1)
    } else {
        room_words
    };
    if words == 0 {
        return Store::PassAll;
    }
    let mut inner =
        BloomFilterInner::with_num_bits(words.saturating_mul(64)).expected_items(hashes.len());
    for &hash in hashes {
        inner.insert_hash(hash);
    }
    Store::Sized(inner)
}

impl BloomFilter {
    /// Creates a new bloom filter with default parameters.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_EXPECTED_ITEMS, DEFAULT_FALSE_POSITIVE_RATE)
    }

    /// Creates a new bloom filter with the specified expected item count and false positive rate.
    pub fn with_capacity(expected_items: u32, false_positive_rate: f32) -> Self {
        Self {
            store: Store::Fixed(
                BloomFilterInner::with_false_pos(false_positive_rate as f64)
                    .expected_items(expected_items as usize),
            ),
            sizing: None,
            count: 0,
            peak: 0,
        }
    }

    /// Amphora (PL.5): a filter sized when its build ends (`seal`), from the distinct keys the
    /// build gave it.
    pub fn sized(sizing: FilterSizing) -> Self {
        Self {
            store: Store::Collecting(Vec::new()),
            sizing: Some(sizing),
            count: 0,
            peak: 0,
        }
    }

    fn insert_hash(&mut self, hash: u64) {
        self.count += 1;
        match &mut self.store {
            Store::Fixed(inner) => {
                inner.insert(&hash);
            }
            Store::Collecting(hashes) => {
                let Some(sizing) = self.sizing else {
                    self.store = Store::PassAll;
                    return;
                };
                // The hashes grow within the filter's memory bound, never past it. A growth may
                // move them, holding the old list and the new at once, so the new list is what
                // the bound leaves beside the old one (QT.5, lane qs5-20): it doubles while three
                // lists' room fits, then takes the rest, and past half the bound it can't grow.
                if hashes.len() == hashes.capacity() {
                    let most = whole_words(sizing.max_bytes);
                    let old = hashes.capacity();
                    let more = old.max(1).min(most.saturating_sub(old.saturating_mul(2)));
                    if more == 0 || hashes.try_reserve_exact(more).is_err() {
                        self.store = Store::PassAll;
                        return;
                    }
                    let moving = old
                        .saturating_add(hashes.capacity())
                        .saturating_mul(size_of::<u64>());
                    self.peak = self.peak.max(moving);
                }
                hashes.push(hash);
            }
            // A key added after the build ended: the filter must never reject it.
            Store::Sized(inner) => {
                inner.insert_hash(hash);
            }
            Store::NoKeys => self.store = Store::PassAll,
            Store::PassAll => {}
        }
    }

    fn contains_hash(&self, hash: u64) -> bool {
        match &self.store {
            Store::Fixed(inner) => inner.contains(&hash),
            Store::Sized(inner) => inner.contains_hash(hash),
            Store::NoKeys => false,
            Store::Collecting(_) | Store::PassAll => true,
        }
    }

    /// Amphora (PL.5): ends a sized filter's build: sizes it from the distinct keys it was given
    /// and fills it. A filter already sealed, or of fixed size, is left as it is. Returns the most
    /// bytes the filter held while sealed (QT.5, lane qs5-19): its key hashes and the finished
    /// filter at once, within its bound (`sized_store`), or the copy a shrink of the hashes makes
    /// if that is more (lane qs5-20); a filter left as it is, its bytes. [Self::peak] counts
    /// its growth's moments too.
    pub fn seal(&mut self) -> usize {
        let (Store::Collecting(hashes), Some(sizing)) = (&mut self.store, self.sizing) else {
            return self.bytes();
        };
        let mut hashes = std::mem::take(hashes);
        hashes.sort_unstable();
        hashes.dedup();
        // The duplicates' room is given back to the filter where the copy a shrink may make fits
        // in the bound beside the hashes it copies.
        let copying = hashes
            .capacity()
            .saturating_add(hashes.len())
            .saturating_mul(size_of::<u64>());
        let shrinks = hashes.len() < hashes.capacity() && copying <= sizing.max_bytes;
        if shrinks {
            hashes.shrink_to_fit();
        }
        let held = hashes.capacity().saturating_mul(size_of::<u64>());
        self.store = sized_store(&hashes, sizing, held);
        let sealing = held.saturating_add(self.bytes());
        let sealing = if shrinks {
            sealing.max(copying)
        } else {
            sealing
        };
        self.peak = self.peak.max(sealing);
        sealing
    }

    /// Amphora (QT.5, lane qs5-20): the most bytes this filter held at once so far: its key
    /// hashes' growths, the old list and the new together, and its sealing ([Self::seal]). A
    /// filter of fixed size, its bytes.
    pub fn peak(&self) -> usize {
        self.peak.max(self.bytes())
    }

    /// Amphora (QT.5, lane qs5-19): whether this is a sized filter whose build has not ended
    /// ([Self::seal]).
    pub fn building(&self) -> bool {
        matches!(self.store, Store::Collecting(_)) && self.sizing.is_some()
    }

    /// Amphora (PL.5): whether this filter rejects any key at all (false when it passes every key).
    pub fn filters(&self) -> bool {
        !matches!(self.store, Store::PassAll | Store::Collecting(_))
    }

    /// The bytes the filter holds.
    pub fn bytes(&self) -> usize {
        match &self.store {
            Store::Fixed(inner) | Store::Sized(inner) => inner.num_bits() / 8,
            // A Vec's bytes fit isize, so this never saturates.
            Store::Collecting(hashes) => hashes.capacity().saturating_mul(size_of::<u64>()),
            Store::NoKeys | Store::PassAll => 0,
        }
    }

    #[inline]
    /// Inserts an i64 value into the bloom filter.
    pub fn insert_i64(&mut self, value: i64) {
        self.insert_hash(hash_of(&value));
    }

    #[inline]
    /// Checks if an i64 value might be in the bloom filter.
    pub fn contains_i64(&self, value: i64) -> bool {
        self.contains_hash(hash_of(&value))
    }

    #[inline]
    /// Inserts a byte slice into the bloom filter.
    pub fn insert_bytes(&mut self, value: &[u8]) {
        self.insert_hash(hash_of(&value));
    }

    #[inline]
    /// Checks if a byte slice might be in the bloom filter.
    pub fn contains_bytes(&self, value: &[u8]) -> bool {
        self.contains_hash(hash_of(&value))
    }

    /// Inserts a Value into the bloom filter.
    /// Safety NOTE: does not accept NULL values.
    pub fn insert_value(&mut self, value: &Value) {
        if matches!(value, Value::Null) {
            self.count += 1;
            return;
        }
        self.insert_hash(hash_of_values(std::iter::once(value)));
    }

    /// Checks if a Value might be in the bloom filter.
    pub fn contains_value(&self, value: &Value) -> bool {
        if matches!(value, Value::Null) {
            return false;
        }
        self.contains_hash(hash_of_values(std::iter::once(value)))
    }

    /// Inserts multiple owned Values as a composite key into the bloom filter.
    /// This is because bloom filters only support a single value insertion, so to handle multi
    /// join-key situations we hash the composite key into a single u64 and then insert that
    pub fn insert_values(&mut self, values: &[&Value]) {
        self.insert_hash(hash_of_values(values.iter().copied()));
    }

    /// Checks if multiple owned Values as a composite key might be in the bloom filter.
    pub fn contains_values(&self, values: &[&Value]) -> bool {
        if values.iter().any(|value| matches!(value, Value::Null)) {
            // if any value is NULL, we can never have a match
            return false;
        }
        self.contains_hash(hash_of_values(values.iter().copied()))
    }

    pub fn count(&self) -> usize {
        self.count
    }
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub fn clear(&mut self) {
        match &mut self.store {
            Store::Fixed(inner) => inner.clear(),
            _ => self.store = Store::Collecting(Vec::new()),
        }
        self.count = 0;
    }
}

impl Default for BloomFilter {
    fn default() -> Self {
        Self::new()
    }
}

/// Hashes an owned Value into the provided hasher.
fn hash_value<H: Hasher>(hasher: &mut H, value: &ValueRef) {
    match value {
        // do nothing for NULLs as we will always return false for set membership
        ValueRef::Null => {}
        ValueRef::Numeric(Numeric::Integer(i)) => {
            // Hash integers in the same bucket as numerically equivalent REALs so
            // bloom-filter membership can never return a false-negative for e.g. 10 vs 10.0.
            let f = *i as f64;
            if (f as i64) == *i && f.is_finite() {
                hash_numeric(hasher, f);
            } else {
                // Fallback to the integer domain when the float representation would lose precision.
                1u8.hash(hasher);
                i.hash(hasher);
            }
        }
        ValueRef::Numeric(Numeric::Float(f)) => {
            hash_numeric(hasher, f64::from(*f));
        }
        ValueRef::Text(s) => {
            3u8.hash(hasher);
            s.as_str().hash(hasher);
        }
        ValueRef::Blob(b) => {
            4u8.hash(hasher);
            b.hash(hasher);
        }
    }
}

/// Hashes numeric values (both INTEGER and REAL) into the same domain to mirror SQLite's
/// numeric comparison semantics (e.g. 10 == 10.0, -0.0 == 0.0).
fn hash_numeric<H: Hasher>(hasher: &mut H, f: f64) {
    const NUMERIC_TAG: u8 = 2;
    let bits = normalized_f64_bits(f);
    NUMERIC_TAG.hash(hasher);
    bits.hash(hasher);
}

#[cfg(test)]
mod bloomtests {
    use super::*;
    use crate::types::Text;

    #[test]
    fn test_bloom_filter_i64() {
        let mut bf = BloomFilter::new();
        bf.insert_i64(1);
        bf.insert_i64(2);
        bf.insert_i64(3);

        // These should definitely be found (no false negatives)
        assert!(bf.contains_i64(1));
        assert!(bf.contains_i64(2));
        assert!(bf.contains_i64(3));
    }

    #[test]
    fn test_bloom_filter_values() {
        let mut bf = BloomFilter::new();

        let int_val = Value::from_i64(42);
        let text_val = Value::Text(Text::new("hello".to_string()));
        let null_val = Value::Null;

        bf.insert_value(&int_val);
        bf.insert_value(&text_val);
        bf.insert_value(&null_val);

        assert!(bf.contains_value(&int_val));
        assert!(bf.contains_value(&text_val));
        // NULLs are not hashed into the filter, so membership should be false.
        assert!(!bf.contains_value(&null_val));
    }

    #[test]
    fn test_bloom_filter_false_positive_rate() {
        // Test that false positive rate is roughly as expected
        let mut bf = BloomFilter::with_capacity(1000, 0.01);

        // Insert 1000 values
        for i in 0..1000 {
            bf.insert_i64(i);
        }

        // Check false positive rate on non-inserted values
        let mut false_positives = 0;
        let test_count = 10000;
        for i in 1000..(1000 + test_count) {
            if bf.contains_i64(i) {
                false_positives += 1;
            }
        }

        // False positive rate should be around 1% (allow some variance)
        let rate = false_positives as f64 / test_count as f64;
        assert!(rate < 0.05, "False positive rate {rate} is too high");
    }

    #[test]
    fn test_bloom_filter_numeric_equivalence() {
        let mut bf = BloomFilter::new();

        // Zero variants should all be found regardless of sign or int/float representation
        let zero_float = Value::from_f64(0.0);
        let zero_neg_float = Value::from_f64(-0.0);
        let zero_int = Value::from_i64(0);
        bf.insert_value(&zero_float);
        assert!(bf.contains_value(&zero_float));
        assert!(bf.contains_value(&zero_neg_float));
        assert!(bf.contains_value(&zero_int));

        // Integer/float representations of the same numeric value should match
        let ten_int = Value::from_i64(10);
        let ten_float = Value::from_f64(10.0);
        bf.insert_value(&ten_int);
        assert!(bf.contains_value(&ten_int));
        assert!(bf.contains_value(&ten_float));

        let neg_ten_float = Value::from_f64(-10.0);
        let neg_ten_int = Value::from_i64(-10);
        bf.insert_value(&neg_ten_float);
        assert!(bf.contains_value(&neg_ten_float));
        assert!(bf.contains_value(&neg_ten_int));
    }

    fn sizing(fp_per_key: f64, max_bytes: usize) -> FilterSizing {
        FilterSizing {
            fp_per_key,
            max_bytes,
        }
    }

    /// PL.5: a sized filter takes its size from the distinct keys its build measured, holds no
    /// false negative, and its false positives stay near the rate derived for those keys.
    #[test]
    fn pl_5_sized_filter_from_measured_keys() {
        let keys = 20_000_i64;
        // Each key twice: the filter is sized by the distinct keys.
        let rate_per_key = 0.01 / keys as f64;
        let mut bf = BloomFilter::sized(sizing(rate_per_key, 1 << 20));
        for i in 0..keys * 2 {
            bf.insert_value(&Value::from_i64(i % keys));
        }
        assert!(!bf.filters());
        bf.seal();
        assert!(bf.filters());
        for i in 0..keys {
            assert!(bf.contains_value(&Value::from_i64(i)));
        }
        let probes = 100_000_i64;
        let false_positives = (keys..keys + probes)
            .filter(|i| bf.contains_value(&Value::from_i64(*i)))
            .count();
        // A rate of 1%: three times it is far outside 100,000 draws' spread.
        assert!(false_positives < 3_000, "{false_positives} false positives");
        assert!(bf.bytes() <= 1 << 20);
    }

    /// PL.5: a rate of one or more is not worth a filter; a build past the memory bound drops
    /// it; a build without keys rejects every key. None ever rejects a key it was given.
    #[test]
    fn pl_5_sized_filter_passes_or_rejects_by_its_measure() {
        let mut costly = BloomFilter::sized(sizing(1.0, 1 << 20));
        for i in 0..10 {
            costly.insert_i64(i);
        }
        costly.seal();
        assert!(!costly.filters());
        assert!(costly.contains_i64(1_000));

        let mut over = BloomFilter::sized(sizing(1e-9, 64));
        for i in 0..1_000 {
            over.insert_i64(i);
            assert!(over.bytes() <= 64);
        }
        over.seal();
        assert!(!over.filters());
        assert!((0..1_000).all(|i| over.contains_i64(i)));

        let mut empty = BloomFilter::sized(sizing(1e-6, 1 << 20));
        empty.seal();
        assert!(!empty.contains_i64(7));

        // The bound limits the rate: tiny per-key rates still fit within it.
        let mut tight = BloomFilter::sized(sizing(1e-12, 4_096));
        for i in 0..2_000 {
            tight.insert_i64(i);
        }
        tight.seal();
        assert!(tight.bytes() <= 4_096);
        assert!((0..2_000).all(|i| tight.contains_i64(i)));
    }

    /// QT.5 (lane qs5-19): a sized filter is built while its build's hashes are still held. Do:
    /// seal filters under a bound of 98,304 bytes whose hashes take 65,536 of it (7,000 keys), whose
    /// duplicates leave room once removed (3,000 keys, each twice), and, under 65,536, whose hashes
    /// would fill all of it (8,192 keys). Expect: the most bytes each held while sealed, its hashes
    /// and the finished filter at once, within its bound; the first two filtering, the last passing
    /// every key, as no filter fits beside its hashes (lane qs5-20: they stop growing at half the
    /// bound, where the next growth's two lists would pass it); none rejecting a key it was given.
    #[test]
    fn qt_5_sealing_holds_the_hashes_and_the_filter_within_the_bound() {
        let bound = 98_304;
        let mut full = BloomFilter::sized(sizing(1e-12, bound));
        for i in 0..7_000 {
            full.insert_i64(i);
        }
        let held = full.seal();
        assert!(held <= bound, "{held} bytes held, the bound {bound}");
        assert!(full.filters());
        assert!((0..7_000).all(|i| full.contains_i64(i)));

        let mut twice = BloomFilter::sized(sizing(1e-12, bound));
        for i in 0..6_000 {
            twice.insert_i64(i % 3_000);
        }
        let held = twice.seal();
        assert!(held <= bound, "{held} bytes held, the bound {bound}");
        assert!(twice.filters());
        assert!((0..3_000).all(|i| twice.contains_i64(i)));

        let mut filled = BloomFilter::sized(sizing(1e-12, 65_536));
        for i in 0..8_192 {
            filled.insert_i64(i);
        }
        let held = filled.seal();
        assert!(held <= 65_536, "{held} bytes held, the bound 65536");
        assert!(!filled.filters());
        assert!((0..8_192).all(|i| filled.contains_i64(i)));
        // Sealed again, it holds what it holds.
        assert_eq!(filled.seal(), filled.bytes());
    }

    /// QT.5 (lane qs5-20, T-QT.5.16): a sized filter's key hashes grow within its bound, the old
    /// list and the new counted together as a growth moves them. Do: give filters under bounds of
    /// 98,304 and 65,536 bytes, and one of an odd 1,000, keys one at a time until they pass every
    /// key or hold 20,000, sealing those that still filter. Expect: the most bytes each held at
    /// once, its growths' two lists included, within its bound; a growth that would pass it
    /// passes every key instead; none rejecting a key it was given.
    #[test]
    fn qt_5_16_the_hashes_grow_within_the_bound_old_and_new_together() {
        for bound in [98_304, 65_536, 1_000] {
            let mut filter = BloomFilter::sized(sizing(1e-12, bound));
            let mut given = 0_i64;
            while given < 20_000 && filter.building() {
                filter.insert_i64(given);
                given += 1;
                assert!(
                    filter.peak() <= bound,
                    "{} bytes held at once after {given} keys, the bound {bound}",
                    filter.peak()
                );
            }
            filter.seal();
            assert!(
                filter.peak() <= bound,
                "{} bytes, the bound {bound}",
                filter.peak()
            );
            assert!((0..given).all(|i| filter.contains_i64(i)));
        }
        // A growth is counted as it moves the list: four keys under a roomy bound grew 0→1→2→4,
        // the last growth holding two words and four.
        let mut small = BloomFilter::sized(sizing(1e-12, 1 << 20));
        for i in 0..4 {
            small.insert_i64(i);
        }
        assert_eq!(small.peak(), 6 * size_of::<u64>());
    }
}
