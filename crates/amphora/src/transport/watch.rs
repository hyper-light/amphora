//! Bounded asynchronous Records waits; socket I/O never runs on the polling thread.
use super::{
    common::{Client, Namespace, encode, read, remaining, send, unavailable},
    endpoint::Stream,
    wire::{Answer, FRAME_MAX, Record, Request, Response, SESSION_MAX, WAIT_MAX},
};
use amphora_api::{self as api, Refusal};
use std::{
    future::Future,
    net::Shutdown,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Waker},
    thread,
    time::Instant,
};

// Match the owner's wait admission budget. A dropped future retains its slot until the
// worker exits; connect/authentication is bounded by two seconds and I/O by SESSION_MAX.
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

struct Permit;
impl Permit {
    fn take() -> Result<Self, Refusal> {
        ACTIVE
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_add(1).filter(|next| *next <= WAIT_MAX)
            })
            .map_err(|_| Refusal::Full { limit: WAIT_MAX })?;
        Ok(Self)
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Default)]
struct State {
    result: Option<Result<api::Rev, Refusal>>,
    waker: Option<Waker>,
    stream: Option<Stream>,
    cancelled: bool,
}

/// One remote Records wait, bounded to thirty seconds and eight concurrent workers.
/// Dropping it interrupts socket I/O without waiting for the owner or joining a worker.
pub struct Watch {
    shared: Arc<Mutex<State>>,
    finished: bool,
}

impl Namespace {
    /// Waits for a durable family revision after `after` without blocking an async executor.
    /// The absolute deadline is capped at the transport's thirty-second session budget.
    pub fn watch(&self, family: &str, after: api::Rev, until: Instant) -> Result<Watch, Refusal> {
        api::family(family)?;
        let until = until.min(
            Instant::now()
                .checked_add(SESSION_MAX)
                .ok_or(Refusal::Timeout)?,
        );
        remaining(until)?;
        let encoded = encode(
            &Request::Records {
                namespace: self.name.clone(),
                op: Record::Wait(family.into(), after),
            },
            FRAME_MAX,
        )?;
        let permit = Permit::take()?;
        let shared = Arc::new(Mutex::new(State::default()));
        let running = Arc::clone(&shared);
        let client = self.client.clone();
        thread::Builder::new()
            .name("amphora.watch".into())
            .spawn(move || {
                let _permit = permit;
                let result = wait(&client, &encoded, until, &running);
                complete(&running, result);
            })
            .map_err(unavailable)?;
        Ok(Watch {
            shared,
            finished: false,
        })
    }
}

fn wait(
    client: &Client,
    encoded: &[u8],
    until: Instant,
    shared: &Mutex<State>,
) -> Result<api::Rev, Refusal> {
    // The shared connection helper bounds connect and authentication together to two
    // seconds. Cancellation during that phase is observed before submitting the wait.
    let mut stream = client.stream(until)?;
    let cancellation = stream.try_clone().map_err(unavailable)?;
    {
        let mut state = shared
            .lock()
            .map_err(|_| unavailable("watch state lock poisoned"))?;
        if state.cancelled {
            return Err(Refusal::Timeout);
        }
        state.stream = Some(cancellation);
    }
    send(&mut stream, encoded, until).map_err(|_| Refusal::UnknownOutcome)?;
    match read::<Response>(&mut stream, until).map_err(|_| Refusal::UnknownOutcome)?? {
        Answer::Rev(revision) => Ok(revision),
        _ => Err(Refusal::UnknownOutcome),
    }
}

fn complete(shared: &Mutex<State>, result: Result<api::Rev, Refusal>) {
    let (waker, stream) = match shared.lock() {
        Ok(mut state) => {
            state.result = Some(result);
            (state.waker.take(), state.stream.take())
        }
        Err(poisoned) => {
            let mut state = poisoned.into_inner();
            state.result = Some(Err(unavailable("watch state lock poisoned")));
            (state.waker.take(), state.stream.take())
        }
    };
    drop(stream);
    if let Some(waker) = waker {
        waker.wake();
    }
}

impl Future for Watch {
    type Output = Result<api::Rev, Refusal>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.finished {
            return Poll::Ready(Err(api::invalid("watch", "already completed")));
        }
        // Publish a waker and inspect the result under one short lock. Completion either
        // observes this waker or leaves a result for this poll, so no wake can be missed.
        let waker = cx.waker().clone();
        let (result, replaced) = match this.shared.lock() {
            Ok(mut state) => match state.result.take() {
                Some(result) => (Some(result), None),
                None => (None, state.waker.replace(waker)),
            },
            Err(_) => {
                this.finished = true;
                return Poll::Ready(Err(unavailable("watch state lock poisoned")));
            }
        };
        drop(replaced);
        match result {
            Some(result) => {
                this.finished = true;
                Poll::Ready(result)
            }
            None => Poll::Pending,
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let (stream, waker) = match self.shared.lock() {
            Ok(mut state) => {
                state.cancelled = true;
                (state.stream.take(), state.waker.take())
            }
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.cancelled = true;
                (state.stream.take(), state.waker.take())
            }
        };
        drop(waker);
        // Shutdown wakes the worker's read on every supported platform. Never perform
        // socket operations while holding the result/waker lock, including during drop.
        if let Some(stream) = stream
            && let Err(error) = stream.shutdown(Shutdown::Both)
        {
            tracing::debug!(target: "amphora.transport", event = "watch_shutdown_failed", %error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{database::Database, transport::Owner};
    use amphora_api::Records;
    use std::{
        fs::File,
        io::Read,
        sync::{Condvar, Weak},
        task::Wake,
        time::Duration,
    };

    // These tests share the deliberately process-wide watch admission budget.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn until() -> Instant {
        Instant::now() + Duration::from_secs(10)
    }

    struct Fixture {
        namespace: Namespace,
        client: Client,
        _owner: Owner,
        _database: Arc<Database>,
        _lock: File,
        _directory: tempfile::TempDir,
    }

    fn fixture() -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("owner");
        let lock = File::create(&path).unwrap();
        lock.try_lock().unwrap();
        // An explicit owner over the real in-memory Database pipeline keeps this test
        // independent of SDK startup selection while exercising authenticated TCP I/O.
        let database = Arc::new(Database::in_memory(until()).unwrap());
        let acquiring = Arc::downgrade(&database);
        let owner = Owner::start(&path, move || {
            acquiring.upgrade().map(|database| (*database).clone())
        })
        .unwrap();
        let client = Client::connect(&path, until()).unwrap();
        let namespace = client.namespace("ledger").unwrap();
        Fixture {
            namespace,
            client,
            _owner: owner,
            _database: database,
            _lock: lock,
            _directory: directory,
        }
    }

    #[derive(Default)]
    struct Notice {
        ready: Mutex<bool>,
        event: Condvar,
    }

    impl Wake for Notice {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            *self.ready.lock().unwrap() = true;
            self.event.notify_all();
        }
    }

    fn wait_for_result(watch: &mut Pin<Box<Watch>>, notice: &Arc<Notice>) -> api::Rev {
        let waker = Waker::from(Arc::clone(notice));
        let mut cx = Context::from_waker(&waker);
        let deadline = until();
        loop {
            if let Poll::Ready(result) = watch.as_mut().poll(&mut cx) {
                return result.unwrap();
            }
            let ready = notice.ready.lock().unwrap();
            let (mut ready, _) = notice
                .event
                .wait_timeout_while(
                    ready,
                    deadline.saturating_duration_since(Instant::now()),
                    |v| !*v,
                )
                .unwrap();
            assert!(*ready, "watch did not publish a result before its deadline");
            *ready = false;
        }
    }

    fn commit(namespace: &Namespace) -> api::Rev {
        namespace
            .commit(
                api::Tx {
                    conds: vec![],
                    writes: vec![api::Write::Put {
                        family: "work".into(),
                        key: b"claim".to_vec(),
                        value: b"durable".to_vec(),
                    }],
                },
                until(),
            )
            .unwrap()
    }

    #[test]
    fn pending_remote_watch_wakes_after_commit_without_holding_the_writer() {
        // CP-T1.5/U05/U06: a remote async wait observes a durable Records revision while
        // unrelated SQL continues through the same owner's real writer pipeline.
        let _serial = SERIAL.lock().unwrap();
        let fixture = fixture();
        let mut watch = Box::pin(fixture.namespace.watch("work", 0, until()).unwrap());
        let notice = Arc::new(Notice::default());
        let waker = Waker::from(Arc::clone(&notice));
        let mut cx = Context::from_waker(&waker);
        assert!(watch.as_mut().poll(&mut cx).is_pending());
        fixture
            .client
            .call(until(), |connection| {
                connection.execute_batch(
                    "CREATE TABLE work(value INTEGER); INSERT INTO work VALUES(7)",
                )?;
                assert_eq!(
                    connection
                        .query_row("SELECT value FROM work", [], |row| row.get::<_, i64>(0))?,
                    7
                );
                Ok(())
            })
            .unwrap();
        let revision = commit(&fixture.namespace);
        assert_eq!(wait_for_result(&mut watch, &notice), revision);
        assert!(matches!(
            watch.as_mut().poll(&mut cx),
            Poll::Ready(Err(Refusal::Invalid { .. }))
        ));
    }

    #[test]
    fn completion_before_poll_does_not_need_a_registered_waker() {
        // CP-T1.5/U05: result publication racing ahead of registration cannot lose readiness.
        let _serial = SERIAL.lock().unwrap();
        let fixture = fixture();
        let revision = commit(&fixture.namespace);
        let mut watch = Box::pin(fixture.namespace.watch("work", 0, until()).unwrap());
        let deadline = until();
        while watch.shared.lock().unwrap().result.is_none() {
            assert!(Instant::now() < deadline, "remote wait did not complete");
            thread::yield_now();
        }
        let notice = Arc::new(Notice::default());
        let waker = Waker::from(Arc::clone(&notice));
        assert_eq!(
            watch.as_mut().poll(&mut Context::from_waker(&waker)),
            Poll::Ready(Ok(revision))
        );
        assert!(!*notice.ready.lock().unwrap());
    }

    #[test]
    fn dropping_remote_waits_interrupts_reads_and_releases_bounded_workers() {
        // CP-T1.5/U06: remote watches refuse overflow and cancellation interrupts I/O;
        // no detached client worker survives merely because the server is still waiting.
        let _serial = SERIAL.lock().unwrap();
        let fixture = fixture();
        let deadline = until();
        let watches: Vec<_> = (0..WAIT_MAX)
            .map(|_| fixture.namespace.watch("work", 0, deadline).unwrap())
            .collect();
        assert!(matches!(
            fixture.namespace.watch("work", 0, deadline),
            Err(Refusal::Full { limit: WAIT_MAX })
        ));
        let mut probe = loop {
            if let Some(stream) = watches[0].shared.lock().unwrap().stream.as_ref() {
                break stream.try_clone().unwrap();
            }
            assert!(Instant::now() < deadline, "wait did not authenticate");
            thread::yield_now();
        };
        let workers: Vec<Weak<Mutex<State>>> = watches
            .iter()
            .map(|watch| Arc::downgrade(&watch.shared))
            .collect();
        drop(watches);
        probe
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        match probe.read(&mut [0]) {
            Ok(0) => (),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotConnected
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::ConnectionReset
                ) => {}
            other => panic!("cancelled wait retained a readable socket: {other:?}"),
        }
        while workers.iter().any(|worker| worker.upgrade().is_some())
            || ACTIVE.load(Ordering::Acquire) != 0
        {
            assert!(Instant::now() < deadline, "cancelled workers did not exit");
            thread::yield_now();
        }
    }
}
