//! The archive's objects as bytes: each is its SHA-256 and then its body, so a restore refuses an
//! object the store gave back other than as it was made (CLAUDE.md §3). Bodies are postcard.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The archive's format: a format value, raised when a body's layout changes.
pub(super) const FORMAT: u8 = 1;

const SUM_BYTES: usize = 32;

/// The log generation a segment's frames are of, as its header names it: SQLite's WAL header's
/// checkpoint sequence and salts (sqlite.org/fileformat2.html, "WAL File Format"), which change
/// with each restart of the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Identity {
    pub seq: u32,
    pub salt1: u32,
    pub salt2: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct StoredMark {
    pub frame: u64,
    pub ns: u64,
    pub unknown_from: Option<u64>,
}

/// A segment: frames `first..` of the log, its header, and the marks of the commits it ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Segment {
    pub format: u8,
    pub generation: u64,
    pub identity: Identity,
    pub first: u64,
    pub header: Vec<u8>,
    pub marks: Vec<StoredMark>,
    pub frames: Vec<u8>,
}

/// A base: its pages are in `chunks` objects of `chunk_bytes` (the last shorter), `file_bytes` in
/// all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Manifest {
    pub format: u8,
    pub id: u64,
    pub start_gen: u64,
    pub end: (u64, u64),
    pub file_bytes: u64,
    pub chunk_bytes: u64,
    pub chunks: u64,
}

/// `body` with its sum before it.
pub(super) fn sealed(body: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(
        body.len()
            .checked_add(SUM_BYTES)
            .ok_or_else(|| anyhow::anyhow!("an archive object's size overflows"))?,
    );
    out.extend_from_slice(&Sha256::digest(body));
    out.extend_from_slice(body);
    Ok(out)
}

/// The body of a sealed object, refused if its sum doesn't hold.
pub(super) fn opened<'a>(key: &str, object: &'a [u8]) -> anyhow::Result<&'a [u8]> {
    let (sum, body) = object
        .split_at_checked(SUM_BYTES)
        .ok_or_else(|| anyhow::anyhow!("archive object {key} is shorter than its sum"))?;
    if Sha256::digest(body).as_slice() != sum {
        anyhow::bail!("archive object {key} doesn't match its sum");
    }
    Ok(body)
}

pub(super) fn encode<T: Serialize>(value: &T) -> anyhow::Result<Vec<u8>> {
    sealed(&postcard::to_allocvec(value)?)
}

pub(super) fn decode<T: for<'de> Deserialize<'de>>(key: &str, object: &[u8]) -> anyhow::Result<T> {
    let value: T = postcard::from_bytes(opened(key, object)?)
        .map_err(|e| anyhow::anyhow!("archive object {key} doesn't decode: {e}"))?;
    Ok(value)
}

/// The bytes a sealed segment takes beyond its frames: what is left of an object for frames.
pub(super) fn segment_overhead(segment: &Segment) -> anyhow::Result<usize> {
    let empty = Segment {
        frames: Vec::new(),
        ..segment.clone()
    };
    // The frames' length prefix grows with them: a varint of a usize is at most 10 bytes
    // (postcard's wire format, "varint(usize)").
    encode(&empty)?
        .len()
        .checked_add(10)
        .ok_or_else(|| anyhow::anyhow!("a segment's size overflows"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-T1.12.P10: an object round-trips; a byte changed is refused.
    #[test]
    fn t_t1_12_p10_sealed_objects() {
        let m = Manifest {
            format: FORMAT,
            id: 1,
            start_gen: 2,
            end: (3, 4),
            file_bytes: 8192,
            chunk_bytes: 4096,
            chunks: 2,
        };
        let mut bytes = encode(&m).unwrap();
        assert_eq!(decode::<Manifest>("k", &bytes).unwrap(), m);
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(decode::<Manifest>("k", &bytes).is_err());
        assert!(opened("k", &[0; 3]).is_err());
    }
}
