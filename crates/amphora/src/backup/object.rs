//! The archive's objects as bytes: each is its SHA-256 and then its body, so a restore refuses an
//! object the store gave back other than as it was made (CLAUDE.md §3). Bodies are postcard.

use amphora_engine::storage::log_segments::SegmentId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::plan::{BaseLack, Link};

/// The archive's format: a format value, raised when a body's layout changes.
pub(super) const FORMAT: u8 = 1;

/// The bytes of a SHA-256 sum (FIPS 180-4): a format value.
pub(super) const SUM_BYTES: usize = 32;

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

/// A segment's fields before its frames, as postcard lays them out: [`Segment`]'s fields in order,
/// its frames last, as their length (a varint, postcard's wire format, "varint(usize)") then their
/// bytes. So a segment read in parts gives its head first and its frames after (T-T1.12.M3).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(super) struct SegmentHead {
    pub format: u8,
    pub generation: u64,
    pub identity: Identity,
    pub first: u64,
    pub header: Vec<u8>,
    pub marks: Vec<StoredMark>,
    /// The bytes of its frames, as their length says.
    pub frames: u64,
}

/// The manifest's own format: a format value. 3 says what the base's file held as its copy began
/// ([`Held`]); 1 and 2 didn't, so no restore takes their bases (lane B26).
pub(super) const MANIFEST_FORMAT: u8 = 3;

/// The log's file a closing segment is in (decision 45).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum LogFile {
    Wal,
    Wal2,
}

impl LogFile {
    pub fn name(self) -> &'static str {
        match self {
            Self::Wal => "-wal",
            Self::Wal2 => "-wal2",
        }
    }
}

impl From<SegmentId> for LogFile {
    fn from(segment: SegmentId) -> Self {
        match segment {
            SegmentId::First => Self::Wal,
            SegmentId::Second => Self::Wal2,
        }
    }
}

/// The log's segment the last switch left (decision 45), as a base's copy began.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Closed {
    pub file: LogFile,
    /// Its generation: its header's checkpoint sequence.
    pub seq: u32,
    /// The frames committed in it, and how many of them the base's file held.
    pub frames: u64,
    pub copied: u64,
}

/// What a base's file held as its copy began (lane B26): every frame committed before the generation
/// it began in, which a restore replays from its first frame, or the base restores short.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Held {
    /// The log's generation the base began in, as its header names it.
    pub own: Identity,
    /// The generation before it; None where no segment held one.
    pub closed: Option<Closed>,
}

impl Held {
    /// What the file is not shown to hold, its generation begun in the archive on `link`: none, it
    /// held every frame before it. A switch is the one way a generation begins whole on the one
    /// before ([`super::Fence`]), so the segment it left is then known, by its last frame.
    pub fn lacks(&self, link: Option<Link>) -> Option<BaseLack> {
        let held = self.closed.map_or(0, |c| c.frames);
        match (link, self.closed) {
            (_, Some(c)) if c.copied < c.frames => Some(BaseLack::Uncopied {
                segment: c.file.name(),
                seq: c.seq,
                frames: c.frames,
                copied: c.copied,
            }),
            (Some(Link::Whole(archived)), _) if archived != held => {
                Some(BaseLack::Before { archived, held })
            }
            _ => None,
        }
    }
}

/// A base: its pages are in `chunks` objects of `chunk_bytes` (the last shorter), `file_bytes` in
/// all. `from`: the base each chunk is stored under, in runs of (base id, chunks), in chunk order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Manifest {
    pub format: u8,
    pub id: u64,
    pub start_gen: u64,
    pub end: (u64, u64),
    pub file_bytes: u64,
    pub chunk_bytes: u64,
    pub chunks: u64,
    pub from: Vec<(u64, u64)>,
    pub held: Held,
}

impl Manifest {
    /// The base each chunk is stored under, chunk by chunk.
    pub fn chunk_bases(&self) -> anyhow::Result<Vec<u64>> {
        let mut out = Vec::with_capacity(usize::try_from(self.chunks)?);
        for &(base, run) in &self.from {
            if u64::try_from(out.len())?.saturating_add(run) > self.chunks {
                anyhow::bail!("base {}'s manifest names more chunks than it has", self.id);
            }
            out.extend(std::iter::repeat_n(base, usize::try_from(run)?));
        }
        if u64::try_from(out.len())? != self.chunks {
            anyhow::bail!("base {}'s manifest names fewer chunks than it has", self.id);
        }
        Ok(out)
    }
}

/// `bases` (one per chunk) as runs of (base, chunks in a row).
pub(super) fn runs(bases: &[u64]) -> Vec<(u64, u64)> {
    let mut out: Vec<(u64, u64)> = Vec::new();
    for &base in bases {
        match out.last_mut() {
            Some((b, n)) if *b == base => *n = n.saturating_add(1),
            _ => out.push((base, 1)),
        }
    }
    out
}

/// The manifest at `key`; None where it is of a format before one that says what its base's file
/// held ([`Held`]): no restore takes that base.
pub(super) fn manifest(key: &str, object: &[u8]) -> anyhow::Result<Option<Manifest>> {
    let body = opened(key, object)?;
    match body.first() {
        Some(&MANIFEST_FORMAT) => postcard::from_bytes(body)
            .map(Some)
            .map_err(|e| anyhow::anyhow!("archive object {key} doesn't decode: {e}")),
        Some(earlier) if (1..MANIFEST_FORMAT).contains(earlier) => Ok(None),
        other => anyhow::bail!("archive object {key} is a manifest of no known format ({other:?})"),
    }
}

/// A through mark ([`super::plan::ThroughKey`]): the log shipped through frame `last` of
/// `generation` held every commit stamped at or before `through`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Through {
    pub format: u8,
    pub generation: u64,
    pub last: u64,
    pub through: u64,
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

/// A sealed object read in parts (T-T1.12.M2): its sum from its first bytes, its body hashed as
/// the parts come, and the sum checked once the last is in ([`Opening::finish`]).
#[derive(Default)]
pub(super) struct Opening {
    sum: Vec<u8>,
    hasher: Sha256,
}

impl Opening {
    /// The body's bytes in the next part, in order: what of it follows the sum.
    pub(super) fn feed<'a>(&mut self, part: &'a [u8]) -> &'a [u8] {
        let want = SUM_BYTES.saturating_sub(self.sum.len()).min(part.len());
        let (head, body) = part.split_at_checked(want).unwrap_or((&[], part));
        self.sum.extend_from_slice(head);
        self.hasher.update(body);
        body
    }

    /// Refused where the parts fed held no whole sum, or the body doesn't match it.
    pub(super) fn finish(self, key: &str) -> anyhow::Result<()> {
        if self.sum.len() < SUM_BYTES {
            anyhow::bail!("archive object {key} is shorter than its sum");
        }
        if self.hasher.finalize().as_slice() != self.sum.as_slice() {
            anyhow::bail!("archive object {key} doesn't match its sum");
        }
        Ok(())
    }
}

pub(super) fn encode<T: Serialize>(value: &T) -> anyhow::Result<Vec<u8>> {
    sealed(&postcard::to_allocvec(value)?)
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

    const OWN: Identity = Identity {
        seq: 6,
        salt1: 2,
        salt2: 3,
    };

    fn held(closed: Option<(u64, u64)>) -> Held {
        Held {
            own: OWN,
            closed: closed.map(|(frames, copied)| Closed {
                file: LogFile::Wal,
                seq: 5,
                frames,
                copied,
            }),
        }
    }

    /// AC-T1.12.4 (lane B26): what a base's file is not shown to hold. Do: compare what manifests say
    /// with how the archive's generation before ended. Expect: a closing segment not copied whole
    /// lacks its later frames, whatever the archive says; one that ended at another frame than the
    /// archive's, or none where the archive names one, lacks the log before; one copied whole that
    /// ends where the archive says lacks nothing, nor does a generation begun on a gap, or the log's
    /// first.
    #[test]
    fn a_base_says_what_its_file_holds() {
        let whole = |last| Some(Link::Whole(last));
        let uncopied = BaseLack::Uncopied {
            segment: "-wal",
            seq: 5,
            frames: 41,
            copied: 20,
        };
        for link in [whole(41), Some(Link::Gap), None] {
            assert_eq!(
                held(Some((41, 20))).lacks(link),
                Some(uncopied.clone()),
                "{link:?}"
            );
        }
        let before = |archived, held| Some(BaseLack::Before { archived, held });
        assert_eq!(held(Some((20, 20))).lacks(whole(41)), before(41, 20));
        assert_eq!(held(None).lacks(whole(41)), before(41, 0));
        assert_eq!(held(Some((41, 41))).lacks(whole(41)), None);
        assert_eq!(held(Some((41, 41))).lacks(Some(Link::Gap)), None);
        assert_eq!(held(None).lacks(Some(Link::Gap)), None);
        assert!(uncopied.to_string().contains("frames 21 to 41 are missing"));
    }

    /// T-T1.12.P10: an object round-trips; a byte changed is refused.
    #[test]
    fn t_t1_12_p10_sealed_objects() {
        let m = Manifest {
            format: MANIFEST_FORMAT,
            id: 1,
            start_gen: 2,
            end: (3, 4),
            file_bytes: 8192,
            chunk_bytes: 4096,
            chunks: 2,
            from: vec![(1, 2)],
            held: held(None),
        };
        let mut bytes = encode(&m).unwrap();
        assert_eq!(manifest("k", &bytes).unwrap(), Some(m));
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(manifest("k", &bytes).is_err());
        assert!(opened("k", &[0; 3]).is_err());
    }

    /// T-T1.12.M2: a sealed object read in parts of any size gives its body in order and checks
    /// its sum; an altered body, or parts that end within the sum, are refused.
    #[test]
    fn t_t1_12_m2_opening_in_parts() {
        let body: Vec<u8> = (0..100u8).collect();
        let sealed = sealed(&body).unwrap();
        for size in [1, 7, SUM_BYTES, SUM_BYTES + 1, sealed.len()] {
            let mut opening = Opening::default();
            let mut out = Vec::new();
            for part in sealed.chunks(size) {
                out.extend_from_slice(opening.feed(part));
            }
            opening.finish("k").unwrap();
            assert_eq!(out, body, "parts of {size}");
        }
        let mut altered = sealed.clone();
        let last = altered.len() - 1;
        altered[last] ^= 1;
        let mut opening = Opening::default();
        for part in altered.chunks(9) {
            opening.feed(part);
        }
        assert!(opening.finish("k").is_err());
        let mut short = Opening::default();
        short.feed(&sealed[..SUM_BYTES - 1]);
        assert!(short.finish("k").is_err());
    }

    /// T-T1.12.M3: a segment's head decodes from its body's first bytes and leaves its frames' bytes
    /// after; a prefix that ends within the head asks for more.
    #[test]
    fn t_t1_12_m3_segment_head() {
        let segment = Segment {
            format: FORMAT,
            generation: 3,
            identity: Identity {
                seq: 1,
                salt1: 2,
                salt2: 3,
            },
            first: 9,
            header: vec![7; 32],
            marks: vec![StoredMark {
                frame: 10,
                ns: 11,
                unknown_from: Some(4),
            }],
            frames: (0..200u8).collect(),
        };
        let body = postcard::to_allocvec(&segment).unwrap();
        let (head, rest) = postcard::take_from_bytes::<SegmentHead>(&body).unwrap();
        assert_eq!(head.first, 9);
        assert_eq!(head.marks, segment.marks);
        assert_eq!(head.header, segment.header);
        assert_eq!(head.frames, 200);
        assert_eq!(rest, segment.frames.as_slice());
        assert!(matches!(
            postcard::take_from_bytes::<SegmentHead>(&body[..40]),
            Err(postcard::Error::DeserializeUnexpectedEnd)
        ));
    }

    /// T-T1.12.I2: a manifest of a format before one that says what its base's file held names no
    /// base a restore takes (lane B26; Amphora is unreleased), and one of no known format is
    /// refused; a manifest's runs give each chunk's base, and runs that don't add up to its chunks
    /// are refused.
    #[test]
    fn t_t1_12_i2_manifest_formats() {
        // Formats 1 and 2 as they were laid out: their fields in order, 2's with its runs last.
        let earlier = (7u64, 1u64, (1u64, 9u64), 3u64 * 4096, 4096u64, 3u64);
        let v1 = encode(&(1u8, earlier)).unwrap();
        let v2 = encode(&(2u8, earlier, vec![(7u64, 3u64)])).unwrap();
        assert_eq!(manifest("k", &v1).unwrap(), None);
        assert_eq!(manifest("k", &v2).unwrap(), None);
        for unknown in [0u8, MANIFEST_FORMAT + 1] {
            assert!(manifest("k", &encode(&(unknown, earlier)).unwrap()).is_err());
        }
        let bases = [3, 3, 9, 3, 9, 9];
        let m = Manifest {
            format: MANIFEST_FORMAT,
            id: 9,
            start_gen: 2,
            end: (2, 1),
            file_bytes: 6 * 4096,
            chunk_bytes: 4096,
            chunks: 6,
            from: runs(&bases),
            held: held(None),
        };
        assert_eq!(m.from, vec![(3, 2), (9, 1), (3, 1), (9, 2)]);
        assert_eq!(m.chunk_bases().unwrap(), bases.to_vec());
        let short = Manifest {
            from: vec![(3, 2)],
            ..m.clone()
        };
        assert!(short.chunk_bases().is_err());
        let long = Manifest {
            from: vec![(3, 7)],
            ..m
        };
        assert!(long.chunk_bases().is_err());
    }
}
