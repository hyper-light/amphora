//! The peer store (CP-T1.12; docs/design/backup.md, "The peer store"): a backup archive's object
//! store kept by another process, served over the owner transport. The process that keeps it runs
//! a [`PeerStore`] over its own store (a folder it owns, `crate::backup::FolderBlobs`); a database
//! backs up to it through a [`PeerBlobs`], the seam's [`Blobs`] on its side, so its archive lives
//! apart from it with no cloud and no mount between them. Each call is one authenticated
//! connection and one request, as a node's pull is; the store's refusals cross typed, as the seam
//! says them.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use amphora_api::Refusal;
use amphora_api::blobs::{BLOB_KEY_MAX, BlobRefusal, Blobs, Listing, check_key};

use super::Error;
use super::common::{Client, Owner, Serves};
use super::wire::{Answer, BlobAnswer, BlobOp, BlobRefused, Request};

/// The bytes one object crossing the transport holds at most: the protocol's bound on a result
/// (`amphora_api::RESULT_MAX`, a format value), so its frame, a mebibyte more, carries it with its
/// key whole.
const OBJECT_BYTES: usize = amphora_api::RESULT_MAX;

/// The bytes one listed object takes in a listing's answer at most: its key, the key's length and
/// its size as postcard's varints (2 bytes for a length up to 1,024, 10 for a `u64`): format values.
const LISTED_BYTES: usize = BLOB_KEY_MAX + 2 + 10;

/// The most objects one listing's answer holds: as many as a result's bytes carry. A listing asked
/// for more says more follow, as the seam lets any store's.
const LISTED_MAX: usize = OBJECT_BYTES / LISTED_BYTES;

/// A peer's object store at an endpoint: the owner transport's listener, record and
/// authentication at `path`, answering each request to `store`. It serves no database. Dropped,
/// it stops listening.
pub struct PeerStore {
    _owner: Owner,
}

impl PeerStore {
    /// An endpoint at `path` serving `store` to the processes that can read its record.
    pub fn start(path: &Path, store: Arc<dyn Blobs>) -> Result<Self, Refusal> {
        Ok(Self {
            _owner: Owner::listen(
                path,
                || None,
                Serves {
                    pulls: None,
                    blobs: Some(store),
                },
            )?,
        })
    }
}

fn size(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

fn count(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

impl From<BlobRefusal> for BlobRefused {
    fn from(refusal: BlobRefusal) -> Self {
        match refusal {
            BlobRefusal::Exists(key) => Self::Exists(key),
            BlobRefusal::NotFound(key) => Self::NotFound(key),
            BlobRefusal::TooLarge {
                what,
                size: s,
                limit,
            } => Self::TooLarge {
                what,
                size: size(s),
                limit: size(limit),
            },
            BlobRefusal::Full { what, limit } => Self::Full {
                what,
                limit: size(limit),
            },
            BlobRefusal::Invalid { what, why } => Self::Invalid { what, why },
            BlobRefusal::Timeout => Self::Timeout,
            BlobRefusal::Unavailable(why) => Self::Unavailable(why),
        }
    }
}

impl From<BlobRefused> for BlobRefusal {
    fn from(refused: BlobRefused) -> Self {
        match refused {
            BlobRefused::Exists(key) => Self::Exists(key),
            BlobRefused::NotFound(key) => Self::NotFound(key),
            BlobRefused::TooLarge { what, size, limit } => Self::TooLarge {
                what,
                size: count(size),
                limit: count(limit),
            },
            BlobRefused::Full { what, limit } => Self::Full {
                what,
                limit: count(limit),
            },
            BlobRefused::Invalid { what, why } => Self::Invalid { what, why },
            BlobRefused::Timeout => Self::Timeout,
            BlobRefused::Unavailable(why) => Self::Unavailable(why),
        }
    }
}

/// `store`'s answer to one request: its most no larger than an object crossing the transport
/// holds, a listing no longer than one answer carries.
pub(super) fn answer(
    store: &dyn Blobs,
    op: BlobOp,
    until: Instant,
) -> Result<BlobAnswer, BlobRefused> {
    Ok(match op {
        BlobOp::ObjectMax => BlobAnswer::ObjectMax(size(store.object_max().min(OBJECT_BYTES))),
        BlobOp::Create { key, bytes } => {
            store.create(&key, &bytes, until)?;
            BlobAnswer::Done
        }
        BlobOp::Get(key) => {
            let bytes = store.get(&key, until)?;
            if bytes.len() > OBJECT_BYTES {
                return Err(BlobRefused::TooLarge {
                    what: format!("object {key}"),
                    size: size(bytes.len()),
                    limit: size(OBJECT_BYTES),
                });
            }
            BlobAnswer::Bytes(bytes)
        }
        BlobOp::List {
            prefix,
            after,
            limit,
        } => {
            let Listing { objects, truncated } = store.list(
                &prefix,
                after.as_deref(),
                count(limit).min(LISTED_MAX),
                until,
            )?;
            BlobAnswer::Listing { objects, truncated }
        }
        BlobOp::Delete(key) => {
            store.delete(&key, until)?;
            BlobAnswer::Done
        }
    })
}

/// A peer's object store, reached at its endpoint ([`PeerStore`]): the seam's [`Blobs`] for a
/// database backed up to it.
pub struct PeerBlobs {
    client: Client,
    object_max: usize,
}

impl PeerBlobs {
    /// The store at the endpoint at `path` (the path its [`PeerStore`] was started at), its most
    /// asked of it once.
    pub fn connect(path: &Path, until: Instant) -> Result<Self, BlobRefusal> {
        let client = Client::at(path.to_path_buf());
        match ask(&client, BlobOp::ObjectMax, false, until)? {
            BlobAnswer::ObjectMax(most) => Ok(Self {
                client,
                object_max: count(most).min(OBJECT_BYTES),
            }),
            _ => Err(not_a_stores()),
        }
    }

    fn ask(&self, op: BlobOp, creating: bool, until: Instant) -> Result<BlobAnswer, BlobRefusal> {
        ask(&self.client, op, creating, until)
    }
}

/// The transport's refusal as the seam's: a deadline as its own, the rest the store unreachable.
fn lost(refusal: Refusal) -> BlobRefusal {
    match refusal {
        Refusal::Timeout => BlobRefusal::Timeout,
        other => BlobRefusal::Unavailable(format!("the peer store: {other}")),
    }
}

fn not_a_stores() -> BlobRefusal {
    BlobRefusal::Unavailable("the peer's answer is not an object store's".into())
}

/// One request and its answer. An answer lost after the request went out leaves a create's
/// outcome unknown: [`BlobRefusal::Timeout`], as the seam says it ("a create may still land"), so
/// the writer reads the object back to learn whether it landed.
fn ask(
    client: &Client,
    op: BlobOp,
    creating: bool,
    until: Instant,
) -> Result<BlobAnswer, BlobRefusal> {
    match client.request(Request::Blob(op), until) {
        Ok(Answer::Blob(answer)) => answer.map_err(BlobRefusal::from),
        Ok(_) => Err(not_a_stores()),
        Err(Error::UnknownOutcome) if creating => Err(BlobRefusal::Timeout),
        Err(Error::UnknownOutcome) => Err(BlobRefusal::Unavailable(
            "the peer store's answer was lost".into(),
        )),
        Err(Error::Refused(refusal)) => Err(lost(refusal)),
    }
}

impl Blobs for PeerBlobs {
    fn object_max(&self) -> usize {
        self.object_max
    }

    fn create(&self, key: &str, bytes: &[u8], until: Instant) -> Result<(), BlobRefusal> {
        check_key(key)?;
        if bytes.len() > self.object_max {
            return Err(BlobRefusal::TooLarge {
                what: "object".into(),
                size: bytes.len(),
                limit: self.object_max,
            });
        }
        let op = BlobOp::Create {
            key: key.to_owned(),
            bytes: bytes.to_vec(),
        };
        match self.ask(op, true, until)? {
            BlobAnswer::Done => Ok(()),
            _ => Err(not_a_stores()),
        }
    }

    fn get(&self, key: &str, until: Instant) -> Result<Vec<u8>, BlobRefusal> {
        check_key(key)?;
        match self.ask(BlobOp::Get(key.to_owned()), false, until)? {
            BlobAnswer::Bytes(bytes) => Ok(bytes),
            _ => Err(not_a_stores()),
        }
    }

    fn list(
        &self,
        prefix: &str,
        after: Option<&str>,
        limit: usize,
        until: Instant,
    ) -> Result<Listing, BlobRefusal> {
        let op = BlobOp::List {
            prefix: prefix.to_owned(),
            after: after.map(str::to_owned),
            limit: size(limit),
        };
        match self.ask(op, false, until)? {
            BlobAnswer::Listing { objects, truncated } => Ok(Listing { objects, truncated }),
            _ => Err(not_a_stores()),
        }
    }

    fn delete(&self, key: &str, until: Instant) -> Result<(), BlobRefusal> {
        check_key(key)?;
        match self.ask(BlobOp::Delete(key.to_owned()), false, until)? {
            BlobAnswer::Done => Ok(()),
            _ => Err(not_a_stores()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use amphora_api::blobs::{SimBlobs, conformance};

    fn until() -> Instant {
        Instant::now().checked_add(crate::transport::HANG).unwrap()
    }

    /// AMPHORA-BLOBS-1 to 7 over the transport, in one process: a peer store over the seam's
    /// simulator passes the seam's conformance suite through its client, and a client's limit past
    /// one answer's is held to it.
    #[test]
    fn amphora_blobs_conformance_through_a_peer_store() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = dir.path().join("store");
        let sim: Arc<dyn Blobs> = Arc::new(SimBlobs::new(16, 64, 1 << 10));
        let _store = PeerStore::start(&endpoint, Arc::clone(&sim)).unwrap();
        let peer = PeerBlobs::connect(&endpoint, until()).unwrap();
        assert_eq!(peer.object_max(), 16);
        conformance(&peer, "suite", until()).unwrap();
        peer.create("many/a", b"1", until()).unwrap();
        let listing = peer.list("many/", None, usize::MAX, until()).unwrap();
        assert_eq!(listing.objects, vec![("many/a".to_owned(), 1)]);
        assert!(!listing.truncated);
        assert_eq!(sim.get("many/a", until()).unwrap(), b"1");
    }

    /// The seam's refusals cross the transport as they were.
    #[test]
    fn a_stores_refusals_cross_typed() {
        for refusal in [
            BlobRefusal::Exists("k".into()),
            BlobRefusal::NotFound("k".into()),
            BlobRefusal::TooLarge {
                what: "object".into(),
                size: 3,
                limit: 2,
            },
            BlobRefusal::Full {
                what: "objects".into(),
                limit: 4,
            },
            BlobRefusal::Invalid {
                what: "key".into(),
                why: "empty".into(),
            },
            BlobRefusal::Timeout,
            BlobRefusal::Unavailable("down".into()),
        ] {
            let wire = postcard::to_allocvec(&BlobRefused::from(refusal.clone())).unwrap();
            let back: BlobRefused = postcard::from_bytes(&wire).unwrap();
            assert_eq!(BlobRefusal::from(back), refusal);
        }
    }
}
