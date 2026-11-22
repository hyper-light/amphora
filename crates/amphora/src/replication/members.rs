//! The group's member bound, stated once in its log the way its class is (docs/design/
//! replication.md, "The member bound stated once per group"; QB.1, lane constants-21): the most
//! members a configuration of the group may name, which every member's core is opened for.
//! hyper-raft stops a member handed a configuration past the bound it was opened for
//! (`Tracker::apply`), so the bound is the group's, read from its log, never a member's own
//! picked number. A group starts with room to grow from its first configuration, and a change
//! that would pass the bound first commits a larger one: the core raises its bound as it takes
//! the entry, before the change behind it takes effect (`hyper_raft::Config::stated_members`).
use crate::db::Db;
use amphora_api::{self as api, Refusal};

/// A bound entry's prefix: a log entry kind of its own, as the class's and the seed's.
const PREFIX: &[u8] = b"amphora.members.v1\0";
const TABLE: &str = "CREATE TABLE IF NOT EXISTS __amphora_member_bound (id INTEGER PRIMARY KEY CHECK(id = 1), bound BLOB NOT NULL)";
/// Format: a bound entry is its prefix and the bound, a little-endian `u64`.
const ENTRY_BYTES: usize = PREFIX.len().saturating_add(std::mem::size_of::<u64>());

/// The bound a group starts with, from the members its first configuration names: twice them,
/// and two at least, room for a member to join a one-member group. Doubling is the geometric
/// growth whose raises cost a constant amount per member added, amortized (Cormen et al.,
/// "Introduction to Algorithms", 3rd ed., §17.4, dynamic tables).
pub(crate) fn first(members: usize) -> usize {
    members.saturating_mul(2).max(2)
}

/// The bound a change raises `bound` to before a configuration of `members`: twice `bound`, or
/// `members` if that is more (§17.4 again).
pub(crate) fn grown(bound: usize, members: usize) -> usize {
    bound.saturating_mul(2).max(members)
}

/// The log entry that states `bound`.
pub(crate) fn entry(bound: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(ENTRY_BYTES);
    bytes.extend_from_slice(PREFIX);
    bytes.extend_from_slice(&u64::try_from(bound).unwrap_or(u64::MAX).to_le_bytes());
    bytes
}

/// Whether a log entry's data (past its stamp) is a bound entry.
pub(super) fn is_bound(data: &[u8]) -> bool {
    data.starts_with(PREFIX)
}

/// The bound a bound entry's data states.
pub(super) fn decode(data: &[u8]) -> Result<usize, Refusal> {
    api::bound("member bound entry", data.len(), ENTRY_BYTES)?;
    let bound = data
        .strip_prefix(PREFIX)
        .and_then(|bound| <[u8; 8]>::try_from(bound).ok())
        .map(u64::from_le_bytes)
        .ok_or_else(|| api::invalid("member bound entry", "the entry is not a bound entry"))?;
    usize::try_from(bound).map_err(|error| Refusal::Unavailable(error.to_string()))
}

/// The bound a stamped log entry states, if it states one: what the core reads as it takes
/// entries (`hyper_raft::Config::stated_members`).
pub(crate) fn stated(entry: &[u8]) -> Option<usize> {
    let (_, data) = super::closed::open(entry).ok()?;
    is_bound(data).then(|| decode(data).ok()).flatten()
}

/// The bound this database's group recorded, if any.
pub(super) fn recorded(db: &Db) -> anyhow::Result<Option<usize>> {
    let mut table = false;
    db.query(
        "SELECT 1 FROM sqlite_schema WHERE name = '__amphora_member_bound'",
        [],
        |_| {
            table = true;
            Ok(false)
        },
    )?;
    if !table {
        return Ok(None);
    }
    let mut bound = None;
    db.query(
        "SELECT bound FROM __amphora_member_bound WHERE id = 1",
        [],
        |row| {
            let bytes = row
                .get_value(0)
                .to_blob()
                .ok_or_else(|| anyhow::anyhow!("the recorded member bound is not a blob"))?;
            let bytes = <[u8; 8]>::try_from(bytes)
                .map_err(|_| anyhow::anyhow!("the recorded member bound is not eight bytes"))?;
            bound = Some(usize::try_from(u64::from_le_bytes(bytes))?);
            Ok(false)
        },
    )?;
    Ok(bound)
}

/// Records `bound` as the group's member bound, in the transaction under way.
pub(super) fn record(db: &mut Db, bound: usize) -> anyhow::Result<()> {
    db.exec(TABLE)?;
    db.query(
        "INSERT INTO __amphora_member_bound(id, bound) VALUES (1, ?1) ON CONFLICT(id) DO UPDATE SET bound = excluded.bound",
        [amphora_engine::Value::from_slice(&u64::try_from(bound)?.to_le_bytes())?],
        |_| Ok(true),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-QB.1.8 (QB.1, lane constants-21): a bound entry reads back as the bound it states, through its
    /// stamp, and anything else is no bound; a group's first bound leaves room to grow, and a
    /// raise always admits the configuration that asked for it. Do: one voter grown to 5, then
    /// 9, raising as a change would pass the bound. Expect: bounds 2, 4, 8, 16, each admitting
    /// the configuration that raised it.
    #[test]
    fn t_qb_1_8_a_bound_entry_states_the_bound_and_growth_always_fits() {
        let stamped = super::super::closed::stamp(7, &entry(9));
        assert_eq!(stated(&stamped), Some(9));
        assert_eq!(stated(&super::super::closed::stamp(7, b"other")), None);
        assert!(decode(b"amphora.members.v1\0short").is_err());
        let mut bound = first(1);
        assert_eq!(bound, 2);
        let mut raises = Vec::new();
        for members in 2..=9 {
            if members > bound {
                bound = grown(bound, members);
                raises.push(bound);
            }
            assert!(members <= bound);
        }
        assert_eq!(raises, vec![4, 8, 16]);
        assert_eq!(grown(2, 40), 40);
    }
}
