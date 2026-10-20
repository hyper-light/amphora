//! Blocking drivers behind a pool of connections: how a million clients reach a database that takes a
//! connection per transaction (a pooler's bounded queue in front of a fixed set of connections).

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use crate::{Done, Driver, Event};

/// A database whose write blocks until it is durable, one transaction on one connection at a time.
pub trait Blocking: Send + Sync + 'static {
    /// The events, durably: returns once they are committed.
    fn write(&self, events: &[Event]) -> anyhow::Result<()>;
    /// A session's latest `limit` events: how many came.
    fn history(&self, session: &str, limit: usize) -> anyhow::Result<usize>;
    /// Events whose text holds `word`, newest first, at most `limit`: how many came.
    fn search(&self, word: &str, limit: usize) -> anyhow::Result<usize>;
    /// What the database says of itself after the run, if it says anything.
    fn stats(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
}

/// A blocking driver's writes, asynchronous: queued (at most `queue`; a sender waits for room) for
/// `connections` threads, each a connection's transactions one after another.
pub struct Pooled<B: Blocking> {
    inner: Arc<B>,
    tx: mpsc::SyncSender<(Vec<Event>, Done)>,
}

impl<B: Blocking> Pooled<B> {
    pub fn new(inner: B, connections: usize, queue: usize) -> anyhow::Result<Self> {
        let inner = Arc::new(inner);
        let (tx, rx) = mpsc::sync_channel::<(Vec<Event>, Done)>(queue.max(1));
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..connections.max(1) {
            let (inner, rx) = (inner.clone(), rx.clone());
            std::thread::Builder::new().name(format!("pool-{i}")).spawn(move || {
                loop {
                    let next = match rx.lock() {
                        Ok(rx) => rx.recv(),
                        Err(_) => return,
                    };
                    let Ok((events, done)) = next else { return };
                    done(inner.write(&events));
                }
            })?;
        }
        Ok(Self { inner, tx })
    }
}

impl<B: Blocking> Driver for Pooled<B> {
    fn write(&self, events: Vec<Event>, done: Done) -> anyhow::Result<()> {
        self.tx.send((events, done)).map_err(|_| anyhow::anyhow!("the pool stopped"))
    }

    fn history(&self, session: &str, limit: usize) -> anyhow::Result<usize> {
        self.inner.history(session, limit)
    }

    fn search(&self, word: &str, limit: usize) -> anyhow::Result<usize> {
        self.inner.search(word, limit)
    }

    fn stats(&self) -> serde_json::Value {
        self.inner.stats()
    }
}
