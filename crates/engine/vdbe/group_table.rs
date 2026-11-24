//! Amphora: GROUP BY's groups in a hash table (Amphora's `docs/design/executor.md`, decision 4).
//!
//! A GROUP BY whose rows don't arrive in group order sorted every row and then aggregated the
//! sorted run of each group. Here each row is aggregated into its group's accumulators where it
//! arrives: the group is found by hashing its keys as the hash join and DISTINCT do (so keys that
//! compare equal hash alike: an integer and the equal real, text under its collation), and the
//! bytecode loads the group's accumulators into its registers for the row's aggregate steps and
//! saves them back. A group keeps the values its first row gave the query's non-aggregate
//! expressions (the GROUP BY keys first), as the sorter path keeps its first sorted row's. At the
//! end the groups are sorted in the order a sorter gives them and drained in that order.
//!
//! The table holds at most a budget of bytes: past it, a row of a group the table doesn't hold is
//! refused, and the bytecode sends the row to the GROUP BY sorter as before. A group is so wholly
//! in one or the other, and the sorter's groups are merged with the table's by key as they are
//! emitted, so the output stays in group order.

use crate::alloc::TryClone;
use crate::translate::collate::CollationSeq;
use crate::types::{compare_immutable, KeyInfo, Value, ValueRef};
use crate::vdbe::hash_table::{
    hash_join_key, hash_single_key_ref, keys_equal_distinct, values_equal_distinct,
};
use crate::vdbe::Register;
use crate::{LimboError, Result};
use smallvec::SmallVec;
use std::cmp::Ordering;

/// Bytes a group costs beyond its values: its metadata and open-addressed hash slot.
const GROUP_OVERHEAD: usize = 96;

/// Sentinel marking an empty slot in the open-addressed index table.
const EMPTY_SLOT: u32 = u32::MAX;

fn value_bytes(v: &Value) -> usize {
    let held = match v.as_ref() {
        ValueRef::Text(t) => t.as_str().len(),
        ValueRef::Blob(b) => b.len(),
        ValueRef::Null | ValueRef::Numeric(_) => 0,
    };
    held.saturating_add(std::mem::size_of::<Value>())
}

/// The groups of `num_values` values that are neither text nor blob and `num_accs` accumulators a
/// table of `budget` bytes holds, as `find` charges them: what the planner counts on grouping in
/// the table before the sorter takes the rest (`docs/design/planner.md` decision 7).
pub fn groups_in_budget(num_values: usize, num_accs: usize, budget: usize) -> usize {
    let per_group = num_values
        .saturating_mul(std::mem::size_of::<Value>())
        .saturating_add(num_accs.saturating_mul(std::mem::size_of::<Register>()))
        .saturating_add(GROUP_OVERHEAD);
    budget.checked_div(per_group).unwrap_or(0)
}

fn missing(what: &str) -> LimboError {
    LimboError::InternalError(format!("group table: {what}"))
}

/// One GROUP BY's groups stored in flat contiguous slabs (`all_values`, `all_accs`) with a direct
/// power-of-two open-addressed index (`slots`) to avoid per-group `Vec` allocations and nested
/// hash maps.
pub struct GroupTable {
    /// Leading values that are the group's keys.
    num_keys: usize,
    /// Fixed number of non-aggregate values stored per group (set on first insert).
    num_values: usize,
    /// Fixed number of accumulators stored per group (set on first insert).
    num_accs: usize,
    /// Number of groups currently stored.
    num_groups: usize,
    collations: Vec<CollationSeq>,
    /// Flat slab of group values: group `g` owns `g * num_values .. (g + 1) * num_values`.
    all_values: Vec<Value>,
    /// Flat slab of group accumulators: group `g` owns `g * num_accs .. (g + 1) * num_accs`.
    all_accs: Vec<Register>,
    /// Precomputed 64-bit key hash per group, parallel to `0..num_groups`.
    hashes: Vec<u64>,
    /// Power-of-two open-addressed linear-probing table mapping `hash & mask -> group id`.
    slots: Vec<u32>,
    bytes: usize,
    budget: usize,
    /// The group the last find loaded, for its save.
    current: Option<u32>,
    /// The groups in output order, once sorted, and the next to drain.
    order: Vec<u32>,
    next: usize,
}

impl GroupTable {
    pub fn new(num_keys: usize, collations: Vec<CollationSeq>, budget: usize) -> Self {
        Self {
            num_keys,
            num_values: 0,
            num_accs: 0,
            num_groups: 0,
            collations,
            all_values: Vec::new(),
            all_accs: Vec::new(),
            hashes: Vec::new(),
            slots: Vec::new(),
            bytes: 0,
            budget,
            current: None,
            order: Vec::new(),
            next: 0,
        }
    }

    /// Clears all groups while retaining backing slab and slot allocations for reuse across
    /// `Statement::reset()` and `GroupClear`.
    pub fn reset_for_reuse(&mut self, num_keys: usize, collations: &[CollationSeq], budget: usize) {
        self.num_keys = num_keys;
        self.num_values = 0;
        self.num_accs = 0;
        self.num_groups = 0;
        self.collations.clear();
        self.collations.extend_from_slice(collations);
        self.all_values.clear();
        self.all_accs.clear();
        self.hashes.clear();
        if !self.slots.is_empty() {
            self.slots.fill(EMPTY_SLOT);
        }
        self.bytes = 0;
        self.budget = budget;
        self.current = None;
        self.order.clear();
        self.next = 0;
    }

    /// Heap bytes currently retained by this group table's backing slabs and slot index.
    pub fn allocated_bytes(&self) -> usize {
        self.all_values
            .capacity()
            .saturating_mul(std::mem::size_of::<Value>())
            .saturating_add(
                self.all_accs
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Register>()),
            )
            .saturating_add(
                self.hashes
                    .capacity()
                    .saturating_mul(std::mem::size_of::<u64>()),
            )
            .saturating_add(
                self.slots
                    .capacity()
                    .saturating_mul(std::mem::size_of::<u32>()),
            )
            .saturating_add(
                self.order
                    .capacity()
                    .saturating_mul(std::mem::size_of::<u32>()),
            )
    }

    /// True when this group table's retained backing capacity fits within
    /// [`crate::vdbe::KEPT_EPHEMERAL_BYTES`] for idle statement reuse.
    pub fn can_reuse(&self) -> bool {
        self.allocated_bytes() <= crate::vdbe::KEPT_EPHEMERAL_BYTES
    }

    /// Clears values and accumulators in place before stashing this table for reuse so owned
    /// text/blob buffers are released immediately on `Statement::reset()`.
    pub fn clear_entries(&mut self) {
        self.num_values = 0;
        self.num_accs = 0;
        self.num_groups = 0;
        self.all_values.clear();
        self.all_accs.clear();
        self.hashes.clear();
        if !self.slots.is_empty() {
            self.slots.fill(EMPTY_SLOT);
        }
        self.bytes = 0;
        self.current = None;
        self.order.clear();
        self.next = 0;
    }

    #[inline(always)]
    fn slot_index(hash: u64, len: usize) -> usize {
        let mask = len.wrapping_sub(1);
        usize::try_from(hash & u64::try_from(mask).unwrap_or(u64::MAX)).unwrap_or(mask)
    }

    fn ensure_slot_capacity(&mut self) -> Result<()> {
        // Maintain <= 70% load factor (Knuth TAOCP vol. 3, §6.4 Algorithm L).
        let needed = self.num_groups.saturating_add(1);
        if !self.slots.is_empty() && needed.saturating_mul(10) <= self.slots.len().saturating_mul(7)
        {
            return Ok(());
        }
        let new_len = if self.slots.is_empty() {
            16usize
        } else {
            self.slots
                .len()
                .checked_mul(2)
                .ok_or(LimboError::IntegerOverflow)?
        };
        let mut new_slots = Vec::new();
        new_slots.try_reserve_exact(new_len)?;
        new_slots.resize(new_len, EMPTY_SLOT);
        let mask = new_len.wrapping_sub(1);
        for (idx, &h) in self.hashes.iter().enumerate() {
            let g = u32::try_from(idx).map_err(|_| LimboError::IntegerOverflow)?;
            let mut pos = Self::slot_index(h, new_len);
            loop {
                let slot = new_slots
                    .get_mut(pos)
                    .ok_or_else(|| missing("slot index out of range"))?;
                if *slot == EMPTY_SLOT {
                    *slot = g;
                    break;
                }
                pos = pos.wrapping_add(1) & mask;
            }
        }
        self.slots = new_slots;
        Ok(())
    }

    #[inline(always)]
    fn group_keys(&self, g: usize) -> Option<&[Value]> {
        let start = g.checked_mul(self.num_values)?;
        let end = start.checked_add(self.num_keys)?;
        self.all_values.get(start..end)
    }

    /// Looks up the group for `keys` (pre-hashed as `hash`), or returns the empty slot index
    /// where a new group should be inserted.
    #[inline]
    fn probe_single_key(&self, key0: ValueRef<'_>, hash: u64) -> Result<Option<u32>> {
        let len = self.slots.len();
        if len == 0 {
            return Ok(None);
        }
        let mask = len.wrapping_sub(1);
        let collation = self
            .collations
            .first()
            .copied()
            .unwrap_or(CollationSeq::Binary);
        let mut pos = Self::slot_index(hash, len);
        for _ in 0..len {
            let g = *self
                .slots
                .get(pos)
                .ok_or_else(|| missing("slot index out of range"))?;
            if g == EMPTY_SLOT {
                return Ok(None);
            }
            let gi = usize::try_from(g).map_err(|_| missing("group index overflow"))?;
            if self.hashes.get(gi).copied() == Some(hash) {
                let start = gi
                    .checked_mul(self.num_values)
                    .ok_or_else(|| missing("group value offset overflow"))?;
                let held0 = self
                    .all_values
                    .get(start)
                    .ok_or_else(|| missing("a group with fewer values than keys"))?;
                if values_equal_distinct(held0.as_ref(), key0, collation) {
                    return Ok(Some(g));
                }
            }
            pos = pos.wrapping_add(1) & mask;
        }
        Ok(None)
    }

    fn probe_multi_keys(&self, keys: &[ValueRef<'_>], hash: u64) -> Result<Option<u32>> {
        let len = self.slots.len();
        if len == 0 {
            return Ok(None);
        }
        let mask = len.wrapping_sub(1);
        let mut pos = Self::slot_index(hash, len);
        for _ in 0..len {
            let g = *self
                .slots
                .get(pos)
                .ok_or_else(|| missing("slot index out of range"))?;
            if g == EMPTY_SLOT {
                return Ok(None);
            }
            let gi = usize::try_from(g).map_err(|_| missing("group index overflow"))?;
            if self.hashes.get(gi).copied() == Some(hash) {
                let held = self
                    .group_keys(gi)
                    .ok_or_else(|| missing("a group with fewer values than keys"))?;
                if keys_equal_distinct(held, keys, &self.collations) {
                    return Ok(Some(g));
                }
            }
            pos = pos.wrapping_add(1) & mask;
        }
        Ok(None)
    }

    #[inline]
    fn insert_slot(&mut self, hash: u64, g: u32) -> Result<()> {
        self.ensure_slot_capacity()?;
        let len = self.slots.len();
        let mask = len.wrapping_sub(1);
        let mut pos = Self::slot_index(hash, len);
        for _ in 0..len {
            let slot = self
                .slots
                .get_mut(pos)
                .ok_or_else(|| missing("slot index out of range"))?;
            if *slot == EMPTY_SLOT {
                *slot = g;
                return Ok(());
            }
            pos = pos.wrapping_add(1) & mask;
        }
        Err(missing("open-addressed group table is full"))
    }

    /// Hot VDBE path: find or insert a group directly from a contiguous slice of `Register`s,
    /// avoiding temporary `SmallVec<[&Value; 8]>` construction per row.
    pub fn find_or_insert_regs(
        &mut self,
        regs: &[Register],
        num_accs: usize,
    ) -> Result<Option<u32>> {
        let (hash, found) = if self.num_keys == 1 {
            let key0 = regs
                .first()
                .ok_or_else(|| missing("fewer values than keys"))?
                .get_value()
                .as_ref();
            let collation = self
                .collations
                .first()
                .copied()
                .unwrap_or(CollationSeq::Binary);
            let hash = hash_single_key_ref(&key0, collation);
            (hash, self.probe_single_key(key0, hash)?)
        } else {
            let keys: SmallVec<[ValueRef; 4]> = regs
                .get(..self.num_keys)
                .ok_or_else(|| missing("fewer values than keys"))?
                .iter()
                .map(|r| r.get_value().as_ref())
                .collect();
            let hash = hash_join_key(&keys, &self.collations);
            (hash, self.probe_multi_keys(&keys, hash)?)
        };
        if let Some(g) = found {
            self.current = Some(g);
            return Ok(Some(g));
        }
        let size = regs
            .iter()
            .map(|r| value_bytes(r.get_value()))
            .fold(GROUP_OVERHEAD, usize::saturating_add)
            .saturating_add(num_accs.saturating_mul(std::mem::size_of::<Register>()));
        if self.bytes.saturating_add(size) > self.budget {
            self.current = None;
            return Ok(None);
        }
        if self.num_groups == 0 {
            self.num_values = regs.len();
            self.num_accs = num_accs;
        } else if self.num_values != regs.len() || self.num_accs != num_accs {
            return Err(missing("group width changed mid-query"));
        }
        let g = u32::try_from(self.num_groups).map_err(|_| LimboError::IntegerOverflow)?;
        self.all_values.try_reserve(regs.len())?;
        self.all_accs.try_reserve(num_accs)?;
        self.hashes.try_reserve(1)?;
        for r in regs {
            self.all_values.push(r.get_value().try_clone()?);
        }
        for _ in 0..num_accs {
            self.all_accs.push(Register::Value(Value::Null));
        }
        self.hashes.push(hash);
        self.insert_slot(hash, g)?;
        self.num_groups = self.num_groups.saturating_add(1);
        self.bytes = self.bytes.saturating_add(size);
        self.current = Some(g);
        Ok(Some(g))
    }

    /// The group whose keys are the first `num_keys` of `values`, made from copies of them with
    /// `num_accs` empty accumulators if there is none; None if there is none and making it would
    /// pass the budget. The group found becomes the current one.
    #[cfg(test)]
    pub fn find_or_insert(&mut self, values: &[&Value], num_accs: usize) -> Result<Option<u32>> {
        let (hash, found) = if self.num_keys == 1 {
            let key0 = values
                .first()
                .ok_or_else(|| missing("fewer values than keys"))?
                .as_ref();
            let hash = hash_join_key(&[key0], &self.collations);
            (hash, self.probe_single_key(key0, hash)?)
        } else {
            let keys: SmallVec<[ValueRef; 4]> = values
                .get(..self.num_keys)
                .ok_or_else(|| missing("fewer values than keys"))?
                .iter()
                .map(|v| v.as_ref())
                .collect();
            let hash = hash_join_key(&keys, &self.collations);
            (hash, self.probe_multi_keys(&keys, hash)?)
        };
        if let Some(g) = found {
            self.current = Some(g);
            return Ok(Some(g));
        }
        let size = values
            .iter()
            .map(|v| value_bytes(v))
            .fold(GROUP_OVERHEAD, usize::saturating_add)
            .saturating_add(num_accs.saturating_mul(std::mem::size_of::<Register>()));
        if self.bytes.saturating_add(size) > self.budget {
            self.current = None;
            return Ok(None);
        }
        if self.num_groups == 0 {
            self.num_values = values.len();
            self.num_accs = num_accs;
        } else if self.num_values != values.len() || self.num_accs != num_accs {
            return Err(missing("group width changed mid-query"));
        }
        let g = u32::try_from(self.num_groups).map_err(|_| LimboError::IntegerOverflow)?;
        self.all_values.try_reserve(values.len())?;
        self.all_accs.try_reserve(num_accs)?;
        self.hashes.try_reserve(1)?;
        for v in values {
            self.all_values.push(v.try_clone()?);
        }
        for _ in 0..num_accs {
            self.all_accs.push(Register::Value(Value::Null));
        }
        self.hashes.push(hash);
        self.insert_slot(hash, g)?;
        self.num_groups = self.num_groups.saturating_add(1);
        self.bytes = self.bytes.saturating_add(size);
        self.current = Some(g);
        Ok(Some(g))
    }

    /// The current group's accumulators.
    #[inline]
    pub fn current_accumulators(&mut self) -> Result<&mut [Register]> {
        let g = self.current.ok_or_else(|| missing("no current group"))?;
        let gi = usize::try_from(g).map_err(|_| missing("the current group is gone"))?;
        let start = gi
            .checked_mul(self.num_accs)
            .ok_or_else(|| missing("accumulator offset overflow"))?;
        let end = start
            .checked_add(self.num_accs)
            .ok_or_else(|| missing("accumulator end overflow"))?;
        self.all_accs
            .get_mut(start..end)
            .ok_or_else(|| missing("the current group is gone"))
    }

    /// Puts the groups in the order `key_info` gives their keys, a sorter's order, to be drained.
    pub fn sort(&mut self, key_info: &[KeyInfo]) -> Result<()> {
        let n = u32::try_from(self.num_groups).map_err(|_| LimboError::IntegerOverflow)?;
        self.order.clear();
        self.order.try_reserve_exact(self.num_groups)?;
        self.order.extend(0..n);
        let (all_values, num_values, num_keys) = (&self.all_values, self.num_values, self.num_keys);
        let keys = |g: u32| {
            usize::try_from(g)
                .ok()
                .and_then(|i| i.checked_mul(num_values))
                .and_then(|start| start.checked_add(num_keys).map(|end| (start, end)))
                .and_then(|(start, end)| all_values.get(start..end))
                .unwrap_or_default()
        };
        self.order.sort_by(|&a, &b| {
            let (ka, kb) = (keys(a), keys(b));
            if ka.len() < key_info.len() || kb.len() < key_info.len() {
                return Ordering::Equal;
            }
            compare_immutable(ka.iter(), kb.iter(), key_info)
        });
        self.next = 0;
        Ok(())
    }

    /// Zero-allocation drain helper: yields mutable slices to the next sorted group's values and
    /// accumulators so the caller can move them directly into VDBE registers.
    pub fn drain_next_slices(
        &mut self,
        bound: Option<&[ValueRef]>,
        key_info: &[KeyInfo],
    ) -> Result<Option<(&mut [Value], &mut [Register])>> {
        let Some(&g) = self.order.get(self.next) else {
            return Ok(None);
        };
        let gi = usize::try_from(g).map_err(|_| missing("a sorted group is gone"))?;
        let val_start = gi
            .checked_mul(self.num_values)
            .ok_or_else(|| missing("group value offset overflow"))?;
        let val_end = val_start
            .checked_add(self.num_values)
            .ok_or_else(|| missing("group value end overflow"))?;
        if let Some(bound) = bound {
            let key_end = val_start
                .checked_add(self.num_keys)
                .ok_or_else(|| missing("group key end overflow"))?;
            let keys = self
                .all_values
                .get(val_start..key_end)
                .ok_or_else(|| missing("a group with fewer values than keys"))?;
            if keys.len() < key_info.len() || bound.len() < key_info.len() {
                return Err(missing("fewer keys than the order compares"));
            }
            if compare_immutable(keys.iter(), bound.iter(), key_info) != Ordering::Less {
                return Ok(None);
            }
        }
        let acc_start = gi
            .checked_mul(self.num_accs)
            .ok_or_else(|| missing("group acc offset overflow"))?;
        let acc_end = acc_start
            .checked_add(self.num_accs)
            .ok_or_else(|| missing("group acc end overflow"))?;
        self.next = self.next.saturating_add(1);
        let values = self
            .all_values
            .get_mut(val_start..val_end)
            .ok_or_else(|| missing("a sorted group is gone"))?;
        let accs = self
            .all_accs
            .get_mut(acc_start..acc_end)
            .ok_or_else(|| missing("a sorted group's accumulators are gone"))?;
        Ok(Some((values, accs)))
    }

    /// The next group in sorted order whose keys order before `bound` (any, with no bound): its
    /// values and its accumulators, moved out of the table (each group is drained once).
    #[cfg(test)]
    pub fn drain_next(
        &mut self,
        bound: Option<&[ValueRef]>,
        key_info: &[KeyInfo],
    ) -> Result<Option<(Vec<Value>, Vec<Register>)>> {
        let Some((values, accs)) = self.drain_next_slices(bound, key_info)? else {
            return Ok(None);
        };
        let out_vals: Vec<Value> = values
            .iter_mut()
            .map(|v| std::mem::replace(v, Value::Null))
            .collect();
        let out_accs: Vec<Register> = accs
            .iter_mut()
            .map(|r| std::mem::replace(r, Register::Value(Value::Null)))
            .collect();
        Ok(Some((out_vals, out_accs)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amphora_engine_parser::ast::SortOrder;

    fn int(i: i64) -> Value {
        Value::from_i64(i)
    }

    fn asc() -> Vec<KeyInfo> {
        vec![KeyInfo {
            sort_order: SortOrder::Asc,
            collation: CollationSeq::Binary,
            nulls_order: None,
        }]
    }

    fn drain_keys(t: &mut GroupTable, bound: Option<&Value>) -> Vec<Value> {
        let bound: Option<Vec<ValueRef>> = bound.map(|b| vec![b.as_ref()]);
        let mut out = Vec::new();
        while let Some((values, _)) = t.drain_next(bound.as_deref(), &asc()).unwrap() {
            out.push(values[0].clone());
        }
        out
    }

    /// Do: find groups for keys 3, 1, 3, 2, 1.0 and NULL twice, then sort and drain before 2,
    /// then the rest. Expect: one group per distinct key (1 and 1.0 one group, the NULLs one, as
    /// GROUP BY groups them), each found again for its later rows; drained NULL and 1 before 2,
    /// then 2 and 3.
    #[test]
    fn rows_find_their_groups_and_groups_drain_in_key_order() {
        let mut t = GroupTable::new(1, vec![CollationSeq::Binary], usize::MAX);
        let keys = [
            int(3),
            int(1),
            int(3),
            int(2),
            Value::from_f64(1.0),
            Value::Null,
            Value::Null,
        ];
        let found: Vec<u32> = keys
            .iter()
            .map(|k| t.find_or_insert(&[k], 1).unwrap().unwrap())
            .collect();
        assert_eq!(found, vec![0, 1, 0, 2, 1, 3, 3]);
        assert_eq!(t.num_groups, 4);
        t.sort(&asc()).unwrap();
        assert_eq!(drain_keys(&mut t, Some(&int(2))), vec![Value::Null, int(1)]);
        assert_eq!(drain_keys(&mut t, None), vec![int(2), int(3)]);
    }

    /// Do: a budget for about two groups; find three keys, then the first again. Expect: the third
    /// refused (None), the first still found: past the budget only new groups are refused.
    #[test]
    fn past_its_budget_the_table_refuses_only_new_groups() {
        let one = GROUP_OVERHEAD + std::mem::size_of::<Value>() + std::mem::size_of::<Register>();
        let mut t = GroupTable::new(1, vec![CollationSeq::Binary], one * 2);
        assert_eq!(t.find_or_insert(&[&int(1)], 1).unwrap(), Some(0));
        assert_eq!(t.find_or_insert(&[&int(2)], 1).unwrap(), Some(1));
        assert_eq!(t.find_or_insert(&[&int(3)], 1).unwrap(), None);
        assert_eq!(t.find_or_insert(&[&int(1)], 1).unwrap(), Some(0));
    }

    /// Review blocker 4 regression: a large group table whose backing slabs exceed
    /// `KEPT_EPHEMERAL_BYTES` must not be retained in idle statement spares across resets.
    #[test]
    fn review_large_group_table_refuses_idle_reuse() {
        let mut t = GroupTable::new(1, vec![CollationSeq::Binary], usize::MAX);
        assert!(t.can_reuse());
        for i in 0..4_000i64 {
            assert!(t.find_or_insert(&[&int(i)], 2).unwrap().is_some());
        }
        assert!(t.allocated_bytes() > crate::vdbe::KEPT_EPHEMERAL_BYTES);
        assert!(
            !t.can_reuse(),
            "large group table ({} bytes) must be dropped on reset",
            t.allocated_bytes()
        );
    }
}
