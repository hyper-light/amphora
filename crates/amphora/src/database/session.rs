//! SQL session Records operations reuse the ordinary Records dispatcher on the writer's
//! current transaction. They neither start a transaction nor publish speculative revisions.

use super::*;

use std::cell::Cell;

thread_local! {
    /// Set while a SQL session [`Database::query_waiting`] runs reads on this thread
    /// (T-T1.10.W11): its Records gets look for a cross-group lock on their row.
    static WATCHING: Cell<bool> = const { Cell::new(false) };
    /// The cross-group key of the first row such a session met under a lock.
    static MET: Cell<Option<Vec<u8>>> = const { Cell::new(None) };
}

/// A SQL session's watch for rows under a cross-group lock, on the thread that runs it, for as
/// long as it lives ([`Database::query_waiting`]).
pub(crate) struct Watch(());

impl Watch {
    pub(crate) fn begin() -> Self {
        MET.set(None);
        WATCHING.set(true);
        Self(())
    }
    /// The cross-group key of the first row the session met under a lock, if it met one.
    pub(crate) fn met(&self) -> Option<Vec<u8>> {
        MET.take()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        WATCHING.set(false);
        MET.set(None);
    }
}

fn sql_error(error: anyhow::Error) -> crate::sql::Error {
    crate::sql::Error::Refusal(refused(&error))
}

pub(crate) fn records_get(
    db: &Db,
    namespace: &str,
    family: &str,
    key: &[u8],
) -> crate::sql::Result<Option<Row>> {
    api::family(namespace)?;
    api::family(family)?;
    api::key(key)?;
    // A session run by Database::query_waiting reads one snapshot, so it can't wait inside
    // itself: a row under a cross-group lock ends this read refused, noted, and the session runs
    // again once the lock resolves (T-T1.10.W11). Any other session reads as today, inside the
    // writer's own transaction too, where a get can't wait for another step.
    if WATCHING.get()
        && let Some(row) =
            crate::xgroup::sql::locked(db, namespace, family, key).map_err(sql_error)?
    {
        MET.set(Some(MET.take().unwrap_or(row)));
        return Err(crate::sql::Error::Refusal(unavailable(
            "a row under a cross-group lock: the session runs again once it resolves",
        )));
    }
    let operation = Operation::Get(
        family.to_owned(),
        key.to_vec(),
        Consistency::Linearizable,
        LockedReads::Committed,
    );
    match apply(db, namespace, &operation).map_err(sql_error)?? {
        Answer::Row(row) => Ok(row),
        _ => Err(crate::sql::Error::Refusal(unavailable(
            "wrong session Records get answer",
        ))),
    }
}

pub(crate) fn records_commit(db: &Db, namespace: &str, tx: Tx) -> crate::sql::Result<Rev> {
    api::family(namespace)?;
    api::transaction(&tx)?;
    let operation = Operation::Mutation(Mutation::Records {
        namespace: namespace.to_owned(),
        tx,
    });
    match apply(db, namespace, &operation).map_err(sql_error)?? {
        Answer::Rev(revision) => Ok(revision),
        _ => Err(crate::sql::Error::Refusal(unavailable(
            "wrong session Records commit answer",
        ))),
    }
}
