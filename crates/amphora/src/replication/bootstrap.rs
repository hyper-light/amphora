//! Explicit full-image adoption, before any resolved page delta (U15 / CP-T1.8).
//!
//! Pending sources retain the original image in their WAL, so elections and unknown proposal
//! outcomes cannot turn a retry into a different seed. Only a committed seed completes adoption.
use amphora_api::{self as api, Refusal};
use amphora_engine::Value;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::db::Db;

const TABLE: &str = "CREATE TABLE IF NOT EXISTS __amphora_baseline (id INTEGER PRIMARY KEY CHECK(id = 1), source TEXT NOT NULL, identity BLOB NOT NULL, complete INTEGER NOT NULL, image BLOB NOT NULL)";
const PREFIX: &[u8] = b"amphora.seed.v1\0";

/// Every initial member must name the same source and original image. The identity is SHA-256
/// of the postcard-encoded, ordered full page image, before adding replication metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Declaration {
    pub source_node: u64,
    pub source_id: [u8; 32],
}

#[derive(Debug)]
pub(super) struct Baseline {
    declaration: Option<Declaration>,
    complete: bool,
    image: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct Seed {
    declaration: Declaration,
    image: Vec<u8>,
}

fn unavailable(error: impl std::fmt::Display) -> Refusal {
    Refusal::Unavailable(error.to_string())
}
fn conflict(why: &str) -> Refusal {
    Refusal::Fenced(format!("baseline import: {why}"))
}

fn table_exists(db: &Db) -> Result<bool, Refusal> {
    let mut found = false;
    db.query("SELECT name FROM sqlite_schema WHERE name = '__amphora_baseline'", [], |_| {
        found = true;
        Ok(false)
    }).map_err(unavailable)?;
    Ok(found)
}

pub(super) fn nonempty(db: &Db) -> Result<bool, Refusal> {
    let mut found = false;
    db.query("SELECT name FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' LIMIT 1", [], |_| {
        found = true;
        Ok(false)
    }).map_err(unavailable)?;
    Ok(found)
}

pub(super) fn identity(image: &[u8]) -> [u8; 32] {
    Sha256::digest(image).into()
}

impl Baseline {
    /// Must run before bootstrap_db or the replica marker modifies the source database.
    pub(super) fn open(db: &mut Db, node: u64, declaration: Option<Declaration>, marked: bool) -> Result<Self, Refusal> {
        if declaration.as_ref().is_some_and(|d| d.source_node == 0) {
            return Err(api::invalid("baseline source", "node ID must be positive"));
        }
        if table_exists(db)? {
            let mut saved = None;
            db.query("SELECT source, identity, complete, image FROM __amphora_baseline WHERE id = 1", [], |row| {
                let source_node = row.get::<String>(0)?.parse::<u64>()?;
                let source_id: [u8; 32] = row.get_value(1).to_blob().ok_or_else(|| anyhow::anyhow!("baseline identity is not a blob"))?.try_into()?;
                let complete = row.get::<i64>(2)? != 0;
                let image = row.get_value(3).to_blob().ok_or_else(|| anyhow::anyhow!("baseline image is not a blob"))?.to_vec();
                saved = Some(Self { declaration: Some(Declaration { source_node, source_id }), complete, image });
                Ok(false)
            }).map_err(unavailable)?;
            let saved = saved.ok_or_else(|| conflict("durable declaration is missing"))?;
            if saved.declaration != declaration {
                return Err(conflict("configured source differs from durable declaration"));
            }
            if !saved.complete && node != saved.declaration.as_ref().map_or(0, |d| d.source_node) && !saved.image.is_empty() {
                return Err(conflict("a follower contains a retained source image"));
            }
            if !saved.image.is_empty() && saved.declaration.as_ref().is_none_or(|d| identity(&saved.image) != d.source_id) {
                return Err(conflict("retained source image identity differs"));
            }
            return Ok(saved);
        }
        if marked {
            if declaration.is_some() {
                return Err(conflict("an existing replica has no matching baseline declaration"));
            }
            return Ok(Self { declaration: None, complete: true, image: Vec::new() });
        }
        let has_data = nonempty(db)?;
        let Some(ref declared) = declaration else {
            if has_data {
                return Err(conflict("nonempty local database requires an explicit source declaration"));
            }
            return Ok(Self { declaration: None, complete: true, image: Vec::new() });
        };
        let image = if node == declared.source_node {
            let pages = db.image_pages(super::SNAPSHOT_BYTES).map_err(unavailable)?;
            let image = postcard::to_allocvec(&pages).map_err(unavailable)?;
            api::bound("baseline image", image.len(), super::SNAPSHOT_BYTES.saturating_add(1 << 20))?;
            if identity(&image) != declared.source_id {
                return Err(conflict("source database identity differs from declaration"));
            }
            image
        } else {
            if has_data {
                return Err(conflict("nonempty follower cannot adopt another source"));
            }
            Vec::new()
        };
        let baseline = Self { declaration, complete: false, image };
        db.transaction(|db| baseline.marker(db)).map_err(unavailable)?;
        db.flusher()().map_err(unavailable)?;
        Ok(baseline)
    }

    pub(super) fn pending(&self) -> bool { !self.complete }

    /// The caller subtracts hyper-durable's entry/context framing from actual log.entry_room().
    pub(super) fn proposal(&self, node: u64, entry_room: usize) -> Result<Option<Vec<u8>>, Refusal> {
        if self.complete { return Ok(None); }
        let declaration = self.declaration.as_ref().ok_or_else(|| conflict("pending declaration is missing"))?;
        if node != declaration.source_node {
            return Err(conflict("only the declared source may propose the initial image"));
        }
        if self.image.is_empty() { return Err(conflict("source image is missing")); }
        let mut bytes = PREFIX.to_vec();
        bytes.extend(postcard::to_allocvec(&Seed { declaration: declaration.clone(), image: self.image.clone() }).map_err(unavailable)?);
        api::bound("baseline log entry", bytes.len(), entry_room.min(super::COMMAND_BYTES.saturating_add(1 << 20)))?;
        Ok(Some(bytes))
    }

    pub(super) fn decode(&self, bytes: &[u8]) -> Result<Option<Vec<(u32, Vec<u8>)>>, Refusal> {
        let Some(payload) = bytes.strip_prefix(PREFIX) else { return Ok(None); };
        api::bound("baseline command", bytes.len(), super::COMMAND_BYTES.saturating_add(1 << 20))?;
        let seed: Seed = postcard::from_bytes(payload).map_err(unavailable)?;
        if self.declaration.as_ref() != Some(&seed.declaration) || identity(&seed.image) != seed.declaration.source_id {
            return Err(conflict("committed seed differs from durable source declaration"));
        }
        let pages: Vec<(u32, Vec<u8>)> = postcard::from_bytes(&seed.image).map_err(unavailable)?;
        super::validate_pages(&pages, super::SNAPSHOT_BYTES)?;
        let mut expected = 1u32;
        for (id, _) in &pages {
            if *id != expected { return Err(conflict("seed image is not a complete contiguous database")); }
            expected = expected.checked_add(1).ok_or_else(|| conflict("seed page number overflow"))?;
        }
        if pages.is_empty() { return Err(conflict("seed image has no database header")); }
        Ok(Some(pages))
    }

    /// Included in the same apply_pages transaction as the seed and replica applied point.
    pub(super) fn completed_marker(&self, db: &Db) -> anyhow::Result<()> {
        Self { declaration: self.declaration.clone(), complete: true, image: Vec::new() }.marker(db)
    }
    pub(super) fn complete(&mut self) { self.complete = true; self.image.clear(); }
    pub(super) fn marker(&self, db: &Db) -> anyhow::Result<()> {
        let Some(declaration) = &self.declaration else { return Ok(()); };
        db.exec(TABLE)?;
        db.query("INSERT INTO __amphora_baseline(id,source,identity,complete,image) VALUES(1,?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET source=excluded.source,identity=excluded.identity,complete=excluded.complete,image=excluded.image", [Value::from_text(declaration.source_node.to_string()), Value::from_slice(&declaration.source_id)?, Value::Integer(i64::from(self.complete)), Value::from_slice(&self.image)?], |_| Ok(true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// U15/CP-T1.8: never silently reinterpret a local nonempty database as an empty replica.
    #[test]
    fn unmarked_nonempty_requires_declaration_and_wrong_source_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Db::open(&dir.path().join("source.db")).unwrap();
        db.exec("CREATE TABLE retained(value TEXT); INSERT INTO retained VALUES('base')").unwrap();
        assert!(matches!(Baseline::open(&mut db, 1, None, false), Err(Refusal::Fenced(_))));
        let image = postcard::to_allocvec(&db.image_pages(super::super::SNAPSHOT_BYTES).unwrap()).unwrap();
        let declaration = Declaration { source_node: 1, source_id: identity(&image) };
        assert!(matches!(Baseline::open(&mut db, 2, Some(declaration.clone()), false), Err(Refusal::Fenced(_))));
        let baseline = Baseline::open(&mut db, 1, Some(declaration.clone()), false).unwrap();
        let seed = baseline.proposal(1, super::super::COMMAND_BYTES).unwrap().unwrap();
        assert!(baseline.decode(&seed).unwrap().is_some());
        drop(db);
        let mut db = Db::open(&dir.path().join("source.db")).unwrap();
        let reopened = Baseline::open(&mut db, 1, Some(declaration.clone()), false).unwrap();
        assert_eq!(seed, reopened.proposal(1, super::super::COMMAND_BYTES).unwrap().unwrap());
        let conflicting = Declaration { source_node: 1, source_id: [0;32] };
        assert!(matches!(Baseline::open(&mut db, 1, Some(conflicting), false), Err(Refusal::Fenced(_))));
    }
}
