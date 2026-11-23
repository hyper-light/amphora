//! Amphora (AUDIT 3.3): a run's side tables (hash tables, group tables, bloom filters, star
//! orders) by their id. The ids are the program's own small dense counters (a hash table's id,
//! a cursor's, an instruction's address), so a slot per id answers a lookup by indexing, where a
//! `HashMap` paid a SipHash for it on every row that reached the opcode.

/// A map from a small dense id to a value: `slots[id]` holds the id's value, if any.
#[derive(Debug)]
pub(crate) struct IdMap<T> {
    slots: Vec<Option<T>>,
    len: usize,
}

impl<T> Default for IdMap<T> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            len: 0,
        }
    }
}

/// An id an [`IdMap`] holds a value under.
pub(crate) trait Id: Copy {
    fn slot(self) -> usize;
}

impl Id for usize {
    fn slot(self) -> usize {
        self
    }
}

impl Id for u32 {
    fn slot(self) -> usize {
        // Every target Amphora builds for has a pointer of at least 32 bits.
        usize::try_from(self).unwrap_or(usize::MAX)
    }
}

impl<T> IdMap<T> {
    pub(crate) const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Drops every value, keeping the slots' allocation for the next run.
    pub(crate) fn clear(&mut self) {
        self.slots.clear();
        self.len = 0;
    }

    pub(crate) fn get<K: Id>(&self, id: &K) -> Option<&T> {
        self.slots.get(id.slot()).and_then(Option::as_ref)
    }

    pub(crate) fn get_mut<K: Id>(&mut self, id: &K) -> Option<&mut T> {
        self.slots.get_mut(id.slot()).and_then(Option::as_mut)
    }

    pub(crate) fn contains_key<K: Id>(&self, id: &K) -> bool {
        self.get(id).is_some()
    }

    pub(crate) fn remove<K: Id>(&mut self, id: &K) -> Option<T> {
        let taken = self.slots.get_mut(id.slot()).and_then(Option::take);
        if taken.is_some() {
            self.len = self.len.saturating_sub(1);
        }
        taken
    }

    pub(crate) fn insert<K: Id>(&mut self, id: K, value: T) -> Option<T> {
        let old = slot_of(&mut self.slots, id.slot()).replace(value);
        if old.is_none() {
            self.len = self.len.saturating_add(1);
        }
        old
    }

    /// The id's value, made by `make` if it has none.
    pub(crate) fn get_or_insert_with<K: Id>(&mut self, id: K, make: impl FnOnce() -> T) -> &mut T {
        let slot = slot_of(&mut self.slots, id.slot());
        if slot.is_none() {
            self.len = self.len.saturating_add(1);
        }
        slot.get_or_insert_with(make)
    }
}

/// The id's slot, made (with every slot before it) if `slots` has none yet.
fn slot_of<T>(slots: &mut Vec<Option<T>>, id: usize) -> &mut Option<T> {
    if id >= slots.len() {
        slots.resize_with(id.saturating_add(1), || None);
    }
    // The resize above made the slot, so `at == id`: the clamp only keeps the index in bounds.
    let at = id.min(slots.len().saturating_sub(1));
    &mut slots[at]
}

impl<T: Default> IdMap<T> {
    pub(crate) fn get_or_default<K: Id>(&mut self, id: K) -> &mut T {
        self.get_or_insert_with(id, T::default)
    }
}

#[cfg(test)]
mod tests {
    use super::IdMap;

    #[test]
    fn audit_3_3_id_map_holds_inserts_removes_and_clears() {
        let mut map: IdMap<String> = IdMap::default();
        assert!(map.is_empty());
        assert_eq!(map.insert(7usize, "a".into()), None);
        assert!(!map.is_empty());
        assert_eq!(map.get(&7usize).map(String::as_str), Some("a"));
        assert_eq!(map.get(&3usize), None);
        assert_eq!(map.get(&700usize), None);
        assert_eq!(map.insert(7usize, "b".into()).as_deref(), Some("a"));
        map.get_or_insert_with(2u32, || "c".into()).push('!');
        assert_eq!(map.get(&2usize).map(String::as_str), Some("c!"));
        assert!(map.contains_key(&2u32));
        assert_eq!(map.remove(&2usize).as_deref(), Some("c!"));
        assert_eq!(map.remove(&2usize), None);
        map.get_or_default(0usize).push('z');
        assert_eq!(map.get_mut(&0usize).map(|v| v.as_str()), Some("z"));
        assert_eq!(map.remove(&0usize).as_deref(), Some("z"));
        assert!(!map.is_empty(), "7 is still held");
        assert_eq!(map.remove(&7usize).as_deref(), Some("b"));
        assert!(map.is_empty());
        map.insert(1usize, "d".into());
        map.clear();
        assert!(map.is_empty());
        assert_eq!(map.get(&1usize), None);
    }
}
