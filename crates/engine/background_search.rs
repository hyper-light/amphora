//! Amphora (PL.8, T-PL.8.10, lane pl8-17): the fast plan first, the exact join search after it.
//!
//! Under `BackgroundSearch::Worker` or `BackgroundSearch::Idle` a root statement is prepared with
//! the fast search: the exact search only where its connected subsets fit the tables' count squared,
//! the joins the greedy search tries (`search_budget::SearchBudget::fast`), the greedy search past
//! it. Its first run starts at once. A statement the greedy search planned hands a `SearchRequest`
//! to the placement the switch names, which plans it again with the rule's whole budget, weighing
//! the fast plan's join orders (`join::keep_carried`, the carried-order path). The exact plan is
//! handed back in the statement's `SearchSlot` only when it is cheaper than the fast plan past the
//! weights' error (`learned_costs::cost_margin`); the statement installs it as its next run starts.
//!
//! The two placements, both woken by an event and never by a timer (docs/design/planner.md,
//! decision pl8-17):
//! - `Worker`: one planner thread per database, started by the first request, sleeping on a
//!   condition variable that each request signals. Its queue is bounded by the connections open to
//!   the database, one pending search each on average: past that the oldest request is dropped,
//!   its statement keeping its fast plan, and counted (`BackgroundSearchStats::dropped`).
//! - `Idle`: the connection itself, when its last statement in progress ends
//!   (`Statement::release_active_root_if_counted`), one search each time it goes idle, so the
//!   step or reset that ended the statement is late by one search at most (lane pl8-18). Its
//!   queue holds one request a statement. A connection that never goes idle never runs its
//!   searches; they stay pending and counted.
//!
//! What a search plans on (lane pl8-18, decision pl8-18): only a SELECT is planned this way
//! (`translate::translate`), whose plan reads nothing of what the connection is doing (a write's
//! reads its nesting and its stack of triggers compiling). Its schema and statistics are the ones
//! the fast plan was made on, taken at hand-off on the statement's thread and immutable after
//! (`SearchRequest::schema`). The connection's settings are read as the search runs, and checked
//! as the statement takes the plan: every setting that shapes a plan moves the connection's
//! generation as it changes, so a plan is taken only while the generation is the one taken at
//! hand-off (`Statement::take_exact_plan`); otherwise it is counted stale and the fast plan stays.
//!
//! Every request ends in exactly one outcome, counted: dropped, abandoned (its statement or
//! connection gone), kept (the fast plan stands), replaced (the exact plan handed back) or failed.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use crate::connection::PrepareOptions;
use crate::statement::StatementOrigin;
use crate::sync::{Arc, Weak};
use crate::translate::optimizer::join::JoinOrders;
use crate::vdbe;
use crate::{Connection, Pager, QueryMode};

/// What the exact searches did, summed over a database's statements
/// (`Connection::background_search_stats`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BackgroundSearchStats {
    /// Statements whose fast plan asked for the exact search.
    pub requested: u64,
    /// Requests dropped from a full worker queue, oldest first, or left when the database closed.
    pub dropped: u64,
    /// Requests whose statement or connection had gone when their turn came.
    pub abandoned: u64,
    /// Searches whose plan was not cheaper past the margin: the fast plan stands.
    pub kept: u64,
    /// Searches whose plan was cheaper past the margin, handed to the statement.
    pub replaced: u64,
    /// Of the replaced, plans the statement didn't take because the connection's settings changed
    /// after hand-off: the search may have read some of them as they were and some as they are.
    pub stale: u64,
    /// Searches that ended in an error: the fast plan stands.
    pub failed: u64,
    /// The longest the worker's queue has been.
    pub longest_queue: u64,
}

#[derive(Debug, Default)]
pub(crate) struct SearchCounters {
    requested: AtomicU64,
    dropped: AtomicU64,
    abandoned: AtomicU64,
    kept: AtomicU64,
    replaced: AtomicU64,
    stale: AtomicU64,
    failed: AtomicU64,
    longest_queue: AtomicU64,
}

impl SearchCounters {
    fn add(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// A replaced plan the statement didn't take: the settings moved after hand-off.
    pub(crate) fn note_stale(&self) {
        Self::add(&self.stale);
    }

    pub(crate) fn stats(&self) -> BackgroundSearchStats {
        BackgroundSearchStats {
            requested: self.requested.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            abandoned: self.abandoned.load(Ordering::Relaxed),
            kept: self.kept.load(Ordering::Relaxed),
            replaced: self.replaced.load(Ordering::Relaxed),
            stale: self.stale.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            longest_queue: self.longest_queue.load(Ordering::Relaxed),
        }
    }
}

/// What a statement's exact search came to.
pub(crate) enum Found {
    /// The fast plan stands: the search's plan was not cheaper past the margin, it failed, or the
    /// request was dropped.
    Kept,
    /// The exact search's plan, cheaper than the fast plan past the margin, and the generation of
    /// the connection's settings its request was handed off at (`SearchRequest::generation`).
    Better {
        program: vdbe::Program,
        generation: u64,
    },
}

/// Where a statement's exact search hands back what it came to; the statement takes it as its
/// next run starts. Filled once.
#[derive(Default)]
pub(crate) struct SearchSlot {
    state: parking_lot::Mutex<SlotState>,
    filled: parking_lot::Condvar,
}

#[derive(Default)]
struct SlotState {
    found: Option<Found>,
    /// When the search came to something, kept after the statement takes it.
    filled_at: Option<Instant>,
}

impl SearchSlot {
    /// Hands back what the search came to; `pub(crate)` for the statement's tests, which fill a
    /// slot themselves to take a plan at a chosen generation.
    pub(crate) fn fill(&self, found: Found) {
        {
            let mut state = self.state.lock();
            state.found = Some(found);
            state.filled_at = Some(Instant::now());
        }
        self.filled.notify_all();
    }

    /// What the search came to, once it has and until it is taken.
    pub(crate) fn take(&self) -> Option<Found> {
        self.state.lock().found.take()
    }

    /// Whether the search has come to something.
    pub(crate) fn is_filled(&self) -> bool {
        self.state.lock().filled_at.is_some()
    }

    /// Waits for the search to come to something and returns when it did, for the tests and the
    /// harness that times the exact plan: woken by `fill`, never by a timer.
    #[cfg(test)]
    pub(crate) fn wait_filled(&self) -> Instant {
        let mut state = self.state.lock();
        loop {
            if let Some(at) = state.filled_at {
                return at;
            }
            self.filled.wait(&mut state);
        }
    }
}

/// A statement's exact search, to be run by the placement the switch names.
pub(crate) struct SearchRequest {
    pub(crate) conn: Weak<Connection>,
    /// The schema and its statistics the fast plan was made on, taken at hand-off: the search plans
    /// on these, never on the connection's schema as it moves while the connection runs.
    pub(crate) schema: Arc<crate::schema::Schema>,
    /// The generation of the connection's settings at hand-off
    /// (`Connection::prepare_context_generation`).
    pub(crate) generation: u64,
    /// The full [`crate::vdbe::PrepareContext`] captured alongside `schema` at hand-off.
    pub(crate) prepare_context: crate::vdbe::PrepareContext,
    pub(crate) pager: Arc<Pager>,
    pub(crate) sql: String,
    pub(crate) mode: QueryMode,
    pub(crate) origin: StatementOrigin,
    pub(crate) expected_runs: Option<u64>,
    /// The fast plan's join orders, which the exact search weighs (`join::keep_carried`).
    pub(crate) fast_orders: Option<Arc<JoinOrders>>,
    /// The fast plan's estimated cost, which the exact plan must beat past the margin.
    pub(crate) fast_cost: Option<f64>,
    pub(crate) slot: Weak<SearchSlot>,
}

impl SearchRequest {
    /// Whether its statement and connection are still there: a statement planned again drops its
    /// old slot, so its older request is no longer live.
    fn live(&self) -> bool {
        self.slot.strong_count() > 0 && self.conn.strong_count() > 0
    }

    /// Ends the request without a search: its statement keeps its fast plan.
    fn drop_unrun(self, counters: &SearchCounters) {
        SearchCounters::add(&counters.dropped);
        if let Some(slot) = self.slot.upgrade() {
            slot.fill(Found::Kept);
        }
    }

    /// Runs the exact search: the statement planned again with the rule's whole budget, carrying
    /// the fast plan's orders, and hands the plan back where it is cheaper past the margin.
    pub(crate) fn run(self, counters: &SearchCounters) {
        let (Some(conn), Some(slot)) = (self.conn.upgrade(), self.slot.upgrade()) else {
            SearchCounters::add(&counters.abandoned);
            return;
        };
        let options = PrepareOptions {
            expected_runs: self.expected_runs,
            carried_join_orders: self.fast_orders.clone(),
            ..PrepareOptions::default()
        };
        let planned = crate::statement::plan_sql_on(
            &conn,
            self.pager.clone(),
            &self.sql,
            self.mode,
            self.origin,
            &options,
            self.schema.clone(),
            self.prepare_context.clone(),
        );
        let margin = conn.db.learned_costs.cost_margin();
        let found = match planned {
            Ok(program) if cheaper_past(program.plan_cost, self.fast_cost, margin) => {
                SearchCounters::add(&counters.replaced);
                Found::Better {
                    program,
                    generation: self.generation,
                }
            }
            Ok(_) => {
                SearchCounters::add(&counters.kept);
                Found::Kept
            }
            Err(err) => {
                tracing::debug!(
                    target: "amphora::join_search",
                    error = %err,
                    "the exact search failed: the fast plan stands"
                );
                SearchCounters::add(&counters.failed);
                Found::Kept
            }
        };
        slot.fill(found);
    }
}

/// Whether the exact plan's estimate is below the fast plan's over `1 + margin`. Without both
/// estimates the fast plan stands: there is nothing to tell them apart by.
fn cheaper_past(exact: Option<f64>, fast: Option<f64>, margin: f64) -> bool {
    let margin = if margin.is_finite() && margin > 0.0 {
        margin
    } else {
        0.0
    };
    match (exact, fast) {
        (Some(exact), Some(fast)) => exact * (1.0 + margin) < fast,
        _ => false,
    }
}

/// What `push_bounded` took off the queue: items no longer live, and the oldest live ones past
/// the capacity.
struct Gone<T> {
    abandoned: Vec<T>,
    dropped: Vec<T>,
}

/// Puts `item` on `queue` after taking off the items `live` refuses; past `capacity` (at least one)
/// the oldest leave too.
fn push_bounded<T>(
    queue: &mut VecDeque<T>,
    item: T,
    capacity: usize,
    live: impl Fn(&T) -> bool,
) -> Gone<T> {
    let mut gone = Gone {
        abandoned: Vec::new(),
        dropped: Vec::new(),
    };
    if queue.iter().any(|queued| !live(queued)) {
        let mut kept = VecDeque::with_capacity(queue.len().saturating_add(1));
        for queued in queue.drain(..) {
            if live(&queued) {
                kept.push_back(queued);
            } else {
                gone.abandoned.push(queued);
            }
        }
        *queue = kept;
    }
    while queue.len() >= capacity.max(1) {
        match queue.pop_front() {
            Some(oldest) => gone.dropped.push(oldest),
            None => break,
        }
    }
    queue.push_back(item);
    gone
}

impl SearchCounters {
    /// Ends what `push_bounded` took off: the abandoned counted, the dropped told.
    fn end(&self, gone: Gone<SearchRequest>) {
        for _ in gone.abandoned {
            Self::add(&self.abandoned);
        }
        for request in gone.dropped {
            request.drop_unrun(self);
        }
    }
}

fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Default)]
struct WorkerState {
    queue: VecDeque<SearchRequest>,
    started: bool,
    closed: bool,
}

#[derive(Default)]
struct WorkerShared {
    state: std::sync::Mutex<WorkerState>,
    wake: std::sync::Condvar,
    counters: SearchCounters,
}

/// A database's planner: the worker that placement `Worker` runs searches on, and the counters of
/// both placements. Dropped with the database, which closes the worker.
#[derive(Default)]
pub(crate) struct Planner {
    shared: std::sync::Arc<WorkerShared>,
}

impl Planner {
    pub(crate) fn counters(&self) -> &SearchCounters {
        &self.shared.counters
    }

    pub(crate) fn note_requested(&self) {
        SearchCounters::add(&self.shared.counters.requested);
    }

    /// Hands `request` to the worker, which wakes on it; the queue keeps at most `capacity`
    /// requests (the connections open to the database), dropping the oldest. Starts the worker on
    /// the first request.
    pub(crate) fn enqueue(&self, request: SearchRequest, capacity: usize) {
        self.note_requested();
        let counters = &self.shared.counters;
        let (gone, start) = {
            let mut state = lock(&self.shared.state);
            if state.closed {
                drop(state);
                request.drop_unrun(counters);
                return;
            }
            let gone = push_bounded(&mut state.queue, request, capacity, SearchRequest::live);
            let len = u64::try_from(state.queue.len()).unwrap_or(u64::MAX);
            counters.longest_queue.fetch_max(len, Ordering::Relaxed);
            let start = !state.started;
            state.started = true;
            (gone, start)
        };
        counters.end(gone);
        if start {
            let shared = self.shared.clone();
            let spawned = std::thread::Builder::new()
                .name("amphora-planner".to_string())
                .spawn(move || worker_loop(&shared));
            if let Err(err) = spawned {
                tracing::warn!(
                    target: "amphora::join_search",
                    error = %err,
                    "the planner worker didn't start: its statements keep their fast plans"
                );
                let queued = {
                    let mut state = lock(&self.shared.state);
                    state.started = false;
                    std::mem::take(&mut state.queue)
                };
                for request in queued {
                    request.drop_unrun(counters);
                }
                return;
            }
        }
        self.shared.wake.notify_one();
    }
}

impl Drop for Planner {
    fn drop(&mut self) {
        let queued = {
            let mut state = lock(&self.shared.state);
            state.closed = true;
            std::mem::take(&mut state.queue)
        };
        self.shared.wake.notify_all();
        for request in queued {
            request.drop_unrun(&self.shared.counters);
        }
    }
}

/// The worker: takes the oldest request, runs it, and sleeps until the next one is handed over or
/// the database closes.
fn worker_loop(shared: &WorkerShared) {
    loop {
        let request = {
            let mut state = lock(&shared.state);
            loop {
                if state.closed {
                    return;
                }
                if let Some(request) = state.queue.pop_front() {
                    break request;
                }
                state = shared
                    .wake
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        };
        request.run(&shared.counters);
    }
}

/// A connection's searches for placement `Idle`, run when its last statement in progress ends.
/// Bounded by the statements prepared on it: a statement planned again drops its older request's
/// slot, and requests whose statements have gone leave as the next one arrives.
#[derive(Default)]
pub(crate) struct IdleSearches {
    queue: parking_lot::Mutex<VecDeque<SearchRequest>>,
    running: AtomicBool,
}

impl IdleSearches {
    pub(crate) fn enqueue(&self, request: SearchRequest, counters: &SearchCounters) {
        SearchCounters::add(&counters.requested);
        let gone = {
            let mut queue = self.queue.lock();
            // One live request a statement: the bound is the statements, never a count of ours.
            let capacity = queue.len().saturating_add(1);
            push_bounded(&mut queue, request, capacity, SearchRequest::live)
        };
        counters.end(gone);
    }

    /// The connection went idle: runs its oldest live search, one, so the call that ended the
    /// connection's last statement returns late by one search at most; the rest wait for the next
    /// time it goes idle (lane pl8-18). Requests whose statements have gone ahead of it leave as
    /// abandoned, which costs no search. One is the least work that still moves the queue on: the
    /// bound is the search, never a count or a time of ours. A search that makes the connection
    /// idle again (none does) doesn't start another inside it.
    pub(crate) fn run(&self, counters: &SearchCounters) {
        if self.running.swap(true, Ordering::AcqRel) {
            return;
        }
        loop {
            let next = self.queue.lock().pop_front();
            let Some(request) = next else {
                break;
            };
            if request.live() {
                request.run(counters);
                break;
            }
            SearchCounters::add(&counters.abandoned);
        }
        self.running.store(false, Ordering::Release);
    }

    /// The searches waiting for the connection to go idle.
    pub(crate) fn pending(&self) -> usize {
        self.queue.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Do: 512 connections hand 4 requests each to a queue bounded by the connections open.
    /// Expect: it never holds more than 512, the 1,536 oldest leave, and the newest 512 stay in
    /// order (T-PL.8.10, lane pl8-17: placement `Worker`'s failure mode, queue growth with many
    /// connections, bounded by dropping the oldest).
    #[test]
    fn t_pl_8_10_the_worker_queue_drops_its_oldest_past_the_connections() {
        let connections = 512usize;
        let mut queue = VecDeque::new();
        let mut dropped = 0usize;
        let mut longest = 0usize;
        for round in 0..4usize {
            for conn in 0..connections {
                let gone = push_bounded(&mut queue, (conn, round), connections, |_| true);
                assert!(gone.abandoned.is_empty());
                dropped += gone.dropped.len();
                longest = longest.max(queue.len());
            }
        }
        assert_eq!(longest, connections);
        assert_eq!(dropped, 3 * connections);
        assert_eq!(queue.front(), Some(&(0, 3)));
        assert_eq!(queue.back(), Some(&(connections - 1, 3)));
    }

    /// Do: requests whose statements have gone wait on a full queue as a new one arrives. Expect:
    /// they leave as abandoned before any live request is dropped.
    #[test]
    fn t_pl_8_10_requests_whose_statements_went_leave_before_live_ones() {
        let mut queue = VecDeque::from(vec![(1, false), (2, true), (3, false)]);
        let gone = push_bounded(&mut queue, (4, true), 3, |item| item.1);
        assert_eq!(gone.abandoned, vec![(1, false), (3, false)]);
        assert!(gone.dropped.is_empty());
        assert_eq!(queue, VecDeque::from(vec![(2, true), (4, true)]));
    }

    /// The exact plan replaces the fast one only past the margin, and never without both
    /// estimates.
    #[test]
    fn t_pl_8_10_the_exact_plan_must_be_cheaper_past_the_margin() {
        assert!(cheaper_past(Some(90.0), Some(100.0), 0.0));
        assert!(!cheaper_past(Some(100.0), Some(100.0), 0.0));
        assert!(!cheaper_past(Some(90.0), Some(100.0), 0.2));
        assert!(cheaper_past(Some(80.0), Some(100.0), 0.2));
        assert!(!cheaper_past(None, Some(100.0), 0.0));
        assert!(!cheaper_past(Some(1.0), None, 0.0));
        assert!(cheaper_past(Some(90.0), Some(100.0), f64::NAN));
    }
}
