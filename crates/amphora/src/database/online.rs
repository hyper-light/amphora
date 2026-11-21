//! Online schema changes by F1's intermediate states (CP-T1.12, T-T1.12.1; docs/design/online-schema.md).
//!
//! An index is built without holding the writer for its table's size: it goes absent, delete-only,
//! write-only, then public (Rae et al., "Online, Asynchronous Schema Change in F1", PVLDB 6(11),
//! 2013, section 4), each step one transaction through the writer. While write-only, writes keep it
//! and batches of a backfill give the rows written before it their entries; the cutover to public
//! changes only the catalog. Every step is recorded in `__amphora_schema_changes` in the same
//! transaction as its effect, so a crash leaves the change at a step, invisible to reads until it is
//! public, and the next call carries it on from there.

use super::*;
use amphora_engine::schema::IndexState;
use std::time::Duration;

/// The tables a change keeps, made by its first step (never at open: a replica on a binary from
/// before them would not make them, as `application.rs` says of its own).
const TABLES: &str = "
CREATE TABLE IF NOT EXISTS __amphora_index_states (name TEXT PRIMARY KEY, state INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS __amphora_schema_changes (name TEXT PRIMARY KEY, sql TEXT NOT NULL, idx TEXT NOT NULL, tbl TEXT NOT NULL, phase INTEGER NOT NULL, after INTEGER, until INTEGER);";

/// The phases a change records (a format value of `__amphora_schema_changes.phase`): its index
/// delete-only, write-only with its backfill under way, and backfilled. Public is no row.
const DELETE_ONLY: i64 = 1;
const WRITE_ONLY: i64 = 2;
const BACKFILLED: i64 = 3;

/// What a refusal says when the table's rows break the unique index a change would add: the
/// change can't finish, and its driver undoes it.
pub const BROKEN: &str = "online schema change's rows";

/// A change asked for: its migration's name and SQL (one `CREATE [UNIQUE] INDEX`), its index and
/// table as SQL names them (case-folded, as the engine's `normalize_ident` folds them).
pub struct Plan {
    name: String,
    sql: String,
    index: String,
    table: String,
}

impl Plan {
    pub fn parse(migration: &Migration) -> Result<Self, Refusal> {
        let refuse = |why: &str| api::invalid("online schema change", why);
        let mut parser = Parser::new(migration.sql.as_bytes());
        let first = parser
            .next_cmd()
            .map_err(|e| refuse(&e.to_string()))?
            .ok_or_else(|| refuse("no statement"))?;
        if parser
            .next_cmd()
            .map_err(|e| refuse(&e.to_string()))?
            .is_some()
        {
            return Err(refuse("one statement, a CREATE INDEX"));
        }
        let Cmd::Stmt(Stmt::CreateIndex {
            if_not_exists,
            idx_name,
            tbl_name,
            using,
            ..
        }) = first
        else {
            return Err(refuse("only CREATE INDEX is built online"));
        };
        // Staging a name an index already has would turn that index's reads off: the change
        // names an index that must not exist, and applies once by its migration's name.
        if if_not_exists {
            return Err(refuse("IF NOT EXISTS: the change applies once by name"));
        }
        if idx_name.db_name.is_some() || using.is_some() {
            return Err(refuse("an index of the main database, without USING"));
        }
        Ok(Self {
            name: migration.name.clone(),
            sql: migration.sql.clone(),
            index: idx_name.name.as_str().to_ascii_lowercase(),
            table: tbl_name.as_str().to_ascii_lowercase(),
        })
    }
}

/// What one step did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stepped {
    /// The migration's name was applied before, with the same SQL.
    Already,
    /// The index is made, empty and delete-only.
    DeleteOnly,
    /// The index is write-only: writes keep it from here.
    WriteOnly,
    /// A backfill batch visited `visited` rows, holding the writer `held` for them.
    Batch { visited: u64, held: Duration },
    /// The index is public: the cutover held the writer `held`.
    Public { held: Duration },
}

/// What a change did, for its caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SchemaChange {
    /// Whether this call made the index public (false: its migration was applied before).
    pub applied: bool,
    /// The backfill's batches and the rows they visited, in this call.
    pub batches: u64,
    pub rows: u64,
    /// How long the cutover to public held the writer (AC-T1.12.2's pause, at the writer).
    pub cutover: Option<Duration>,
}

/// How many rows the next backfill batch takes, learned from the batches run so far as the backfill
/// runs (CP-T1.12): a batch holds the writer for as long as the one before it waited for the
/// writer and its flush, so the backfill takes at most half the writer's time, whatever the load
/// and the machine. A batch of one row starts it (the least that moves a backfill); each next is
/// the last's rows scaled by its measured wait over its measured hold, at most twice the last's (a
/// run-out doubling, as decision 9's census grows), at least one row. Too many dirty pages for one
/// transaction halves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Sizer {
    rows: u64,
}

impl Sizer {
    pub(crate) fn new() -> Self {
        Self { rows: 1 }
    }

    pub(crate) fn rows(&self) -> u64 {
        self.rows
    }

    /// A batch visited `visited` rows, held the writer `held` and waited `waited` besides.
    pub(crate) fn learn(&mut self, visited: u64, held: Duration, waited: Duration) {
        if visited == 0 {
            return;
        }
        let most = visited.saturating_mul(2);
        let held = held.as_nanos();
        let want = if held == 0 {
            most
        } else {
            let scaled = u128::from(visited)
                .saturating_mul(waited.as_nanos())
                .checked_div(held)
                .unwrap_or(u128::MAX);
            u64::try_from(scaled).unwrap_or(u64::MAX)
        };
        self.rows = want.clamp(1, most.max(1));
    }

    /// A batch of this size was too large for one transaction: half of it, or None at one row.
    pub(crate) fn halve(&mut self) -> Option<u64> {
        if self.rows <= 1 {
            return None;
        }
        self.rows = self.rows.checked_div(2).unwrap_or(1).max(1);
        Some(self.rows)
    }
}

/// The change's record: its SQL, phase, the last rowid its backfill reached and the largest rowid
/// its table held when the index went write-only.
struct Record {
    sql: String,
    phase: i64,
    after: Option<i64>,
    until: Option<i64>,
}

fn optional(row: &amphora_engine::Row, n: usize) -> anyhow::Result<Option<i64>> {
    match row.get::<&EValue>(n).map_err(unavailable)? {
        EValue::Null => Ok(None),
        _ => Ok(Some(row.get::<i64>(n).map_err(unavailable)?)),
    }
}

fn record(db: &Db, plan: &Plan) -> anyhow::Result<Option<Record>> {
    let mut kept = false;
    db.query(
        "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = '__amphora_schema_changes'",
        [],
        |_| {
            kept = true;
            Ok(false)
        },
    )?;
    if !kept {
        return Ok(None);
    }
    let mut found = None;
    db.query(
        "SELECT sql, phase, after, until FROM __amphora_schema_changes WHERE name = ?1",
        [text(&plan.name)],
        |row| {
            found = Some(Record {
                sql: row.get::<String>(0).map_err(unavailable)?,
                phase: row.get::<i64>(1).map_err(unavailable)?,
                after: optional(row, 2)?,
                until: optional(row, 3)?,
            });
            Ok(false)
        },
    )?;
    Ok(found)
}

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn stored(state: IndexState) -> anyhow::Result<EValue> {
    state
        .stored()
        .map(EValue::from_i64)
        .ok_or_else(|| anyhow::anyhow!("a public index has no state row"))
}

/// One step of `plan`'s change, in the caller's write transaction: the next its record says.
pub fn step(db: &Db, plan: &Plan, rows: u64) -> anyhow::Result<Stepped> {
    let mut applied = None;
    db.query(
        "SELECT sql FROM __amphora_migrations WHERE name = ?1",
        [text(&plan.name)],
        |row| {
            applied = Some(row.get::<String>(0).map_err(unavailable)?);
            Ok(false)
        },
    )?;
    if let Some(sql) = applied {
        if sql == plan.sql {
            return Ok(Stepped::Already);
        }
        return Err(api::invalid("migration", "the applied name has different SQL").into());
    }
    let Some(record) = record(db, plan)? else {
        return begin(db, plan);
    };
    if record.sql != plan.sql {
        return Err(api::invalid(
            "migration",
            "a change of this name is under way with different SQL",
        )
        .into());
    }
    match record.phase {
        DELETE_ONLY => write_only(db, plan),
        WRITE_ONLY => backfill(db, plan, record.after, record.until, rows),
        BACKFILLED => cutover(db, plan),
        phase => Err(anyhow::anyhow!(
            "__amphora_schema_changes: {} is at phase {phase}",
            plan.name
        )),
    }
}

/// Absent to delete-only: the name staged, then the index made under it, empty.
fn begin(db: &Db, plan: &Plan) -> anyhow::Result<Stepped> {
    db.exec(TABLES)?;
    db.query(
        "INSERT INTO __amphora_schema_changes(name, sql, idx, tbl, phase) VALUES (?1, ?2, ?3, ?4, ?5)",
        [
            text(&plan.name),
            text(&plan.sql),
            text(&plan.index),
            text(&plan.table),
            EValue::from_i64(DELETE_ONLY),
        ],
        |_| Ok(true),
    )?;
    db.query(
        "INSERT INTO __amphora_index_states(name, state) VALUES (?1, ?2)",
        [text(&plan.index), stored(IndexState::DeleteOnly)?],
        |_| Ok(true),
    )?;
    db.restage_schema()?;
    db.exec(&plan.sql)?;
    Ok(Stepped::DeleteOnly)
}

/// Delete-only to write-only. In F1 every server reaches delete-only before any goes on; here every
/// write runs on the writer's schema, which this transaction moves, so the drain is immediate
/// (docs/design/online-schema.md says when that stops holding). The rows the backfill must reach
/// are those up to the largest rowid now: a row written after gets its entry from its write.
fn write_only(db: &Db, plan: &Plan) -> anyhow::Result<Stepped> {
    db.query(
        "UPDATE __amphora_index_states SET state = ?2 WHERE name = ?1",
        [text(&plan.index), stored(IndexState::WriteOnly)?],
        |_| Ok(true),
    )?;
    let mut until = None;
    db.query_owned(
        &format!("SELECT max(rowid) FROM {}", quoted(&plan.table)),
        [],
        |row| {
            until = optional(row, 0)?;
            Ok(false)
        },
    )?;
    db.query(
        "UPDATE __amphora_schema_changes SET phase = ?2, until = ?3 WHERE name = ?1",
        [
            text(&plan.name),
            EValue::from_i64(WRITE_ONLY),
            until.map_or(EValue::Null, EValue::from_i64),
        ],
        |_| Ok(true),
    )?;
    db.restage_schema()?;
    Ok(Stepped::WriteOnly)
}

/// One backfill batch of at most `rows` rows after `after`; the last marks the change backfilled.
fn backfill(
    db: &Db,
    plan: &Plan,
    after: Option<i64>,
    until: Option<i64>,
    rows: u64,
) -> anyhow::Result<Stepped> {
    let reached = |at: Option<i64>| match (at, until) {
        (_, None) => true,
        (Some(at), Some(until)) => at >= until,
        (None, Some(_)) => false,
    };
    let (last, visited, held) = if reached(after) {
        (after, 0, Duration::ZERO)
    } else {
        let began = Instant::now();
        let (last, visited) = db
            .index_backfill(&plan.index, after, rows)
            .map_err(|error| backfill_refusal(plan, error))?;
        (last.or(after), visited, began.elapsed())
    };
    let done = visited < rows || reached(last);
    db.query(
        "UPDATE __amphora_schema_changes SET phase = ?2, after = ?3 WHERE name = ?1",
        [
            text(&plan.name),
            EValue::from_i64(if done { BACKFILLED } else { WRITE_ONLY }),
            last.map_or(EValue::Null, EValue::from_i64),
        ],
        |_| Ok(true),
    )?;
    Ok(Stepped::Batch { visited, held })
}

/// A backfill's failure as its driver acts on it: rows that break the unique index the change
/// would add ([`BROKEN`]), a batch too large for one transaction (halved), or the error itself.
fn backfill_refusal(plan: &Plan, error: anyhow::Error) -> anyhow::Error {
    match error.downcast_ref::<amphora_engine::LimboError>() {
        Some(amphora_engine::LimboError::Constraint(why)) => {
            api::invalid(BROKEN, &format!("{} can't be unique: {why}", plan.index)).into()
        }
        Some(amphora_engine::LimboError::TransactionPageLimit { limit, attempted }) => {
            Refusal::TooLarge {
                what: "online schema change batch dirty pages".into(),
                size: *attempted,
                limit: *limit,
            }
            .into()
        }
        _ => error,
    }
}

/// Write-only to public: the cutover. Only the catalog changes, whatever the table's size.
fn cutover(db: &Db, plan: &Plan) -> anyhow::Result<Stepped> {
    let began = Instant::now();
    db.query(
        "DELETE FROM __amphora_index_states WHERE name = ?1",
        [text(&plan.index)],
        |_| Ok(true),
    )?;
    db.restage_schema()?;
    finish(db, plan)?;
    db.query(
        "INSERT INTO __amphora_migrations(name, sql) VALUES (?1, ?2)",
        [text(&plan.name), text(&plan.sql)],
        |_| Ok(true),
    )?;
    Ok(Stepped::Public {
        held: began.elapsed(),
    })
}

fn finish(db: &Db, plan: &Plan) -> anyhow::Result<()> {
    db.query(
        "DELETE FROM __amphora_schema_changes WHERE name = ?1",
        [text(&plan.name)],
        |_| Ok(true),
    )
}

/// Undoes `plan`'s change, at whatever step it stands: its index dropped, its records gone, the
/// old schema whole. Nothing to undo when it never began.
pub fn abandon(db: &Db, plan: &Plan) -> anyhow::Result<()> {
    if record(db, plan)?.is_none() {
        return Ok(());
    }
    db.query(
        "DELETE FROM __amphora_index_states WHERE name = ?1",
        [text(&plan.index)],
        |_| Ok(true),
    )?;
    db.restage_schema()?;
    db.exec(&format!("DROP INDEX IF EXISTS {}", quoted(&plan.index)))?;
    finish(db, plan)
}

impl Database {
    /// Builds the index `migration` creates (one `CREATE [UNIQUE] INDEX`) online, by F1's states
    /// (CP-T1.12, docs/design/online-schema.md): writes go on while its backfill runs, and the
    /// writer is held for the cutover to public only as long as a catalog change takes. Applied
    /// once by name, as [`Database::migrate`]. A deadline that passes leaves the change at a
    /// step, invisible to reads; calling again with the same migration carries it on. Rows that
    /// break a unique index undo the change and are refused.
    pub fn change_schema(
        &self,
        migration: Migration,
        until: Instant,
    ) -> Result<SchemaChange, Refusal> {
        api::family(&migration.name)?;
        validate_sql(&migration.sql, false)?;
        let plan = Arc::new(Plan::parse(&migration)?);
        let database = self.resolve(until)?;
        if database.remote_client().is_some() {
            return Err(api::invalid(
                "online schema change",
                "it runs on the database's owner",
            ));
        }
        let mut sizer = Sizer::new();
        let mut change = SchemaChange::default();
        // Each step moves the change on (a phase, or its backfill's rowid), and every call
        // refuses past `until`: the loop ends on the change's end or the deadline.
        loop {
            let rows = sizer.rows();
            let stepping = Arc::clone(&plan);
            let began = Instant::now();
            let stepped = database.call_db(until, move |db| step(db, &stepping, rows));
            let took = began.elapsed();
            match stepped {
                Ok(Stepped::Already) => return Ok(change),
                Ok(Stepped::Public { held }) => {
                    change.applied = true;
                    change.cutover = Some(held);
                    return Ok(change);
                }
                Ok(Stepped::Batch { visited, held }) => {
                    if visited > 0 {
                        change.batches = change.batches.saturating_add(1);
                        change.rows = change.rows.saturating_add(visited);
                        sizer.learn(visited, held, took.saturating_sub(held));
                    }
                }
                Ok(Stepped::DeleteOnly | Stepped::WriteOnly) => {}
                Err(refusal @ Refusal::TooLarge { .. }) => {
                    if sizer.halve().is_none() {
                        return Err(refusal);
                    }
                }
                Err(Refusal::Invalid { what, why }) if what == BROKEN => {
                    let undoing = Arc::clone(&plan);
                    database.call_db(until, move |db| abandon(db, &undoing))?;
                    return Err(Refusal::Invalid { what, why });
                }
                Err(refusal) => return Err(refusal),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t_t1_12_sizer_starts_at_a_row_and_at_most_doubles() {
        // CP-T1.12: a batch that held the writer 1 ms and waited 10 ms grows only to twice its
        // rows; one that held 10 ms and waited 1 ms shrinks to a tenth; never under a row.
        let mut sizer = Sizer::new();
        assert_eq!(sizer.rows(), 1);
        sizer.learn(1, Duration::from_millis(1), Duration::from_millis(10));
        assert_eq!(sizer.rows(), 2);
        sizer.learn(1000, Duration::from_millis(10), Duration::from_millis(1));
        assert_eq!(sizer.rows(), 100);
        sizer.learn(100, Duration::from_millis(10), Duration::ZERO);
        assert_eq!(sizer.rows(), 1);
    }

    #[test]
    fn t_t1_12_sizer_holds_the_writer_as_long_as_it_waited() {
        // CP-T1.12: a batch that held the writer 4 ms and waited 3 ms besides asks 3/4 of its rows
        // next; one that held 2 ms and waited 3 ms asks 3/2 of them.
        let mut sizer = Sizer::new();
        sizer.learn(400, Duration::from_millis(4), Duration::from_millis(3));
        assert_eq!(sizer.rows(), 300);
        sizer.learn(300, Duration::from_millis(2), Duration::from_millis(3));
        assert_eq!(sizer.rows(), 450);
    }

    #[test]
    fn t_t1_12_sizer_halves_to_a_row_then_refuses() {
        let mut sizer = Sizer::new();
        sizer.learn(3, Duration::ZERO, Duration::from_millis(1));
        assert_eq!(sizer.rows(), 6);
        assert_eq!(sizer.halve(), Some(3));
        assert_eq!(sizer.halve(), Some(1));
        assert_eq!(sizer.halve(), None);
    }

    #[test]
    fn t_t1_12_a_plan_is_one_create_index_without_if_not_exists() {
        let plan = |sql: &str| {
            Plan::parse(&Migration {
                name: "m".into(),
                sql: sql.into(),
            })
        };
        let made = plan("CREATE UNIQUE INDEX \"Ix\" ON T(a)").unwrap();
        assert_eq!((made.index.as_str(), made.table.as_str()), ("ix", "t"));
        assert!(plan("CREATE INDEX IF NOT EXISTS ix ON t(a)").is_err());
        assert!(plan("CREATE INDEX ix ON t(a); CREATE INDEX iy ON t(b)").is_err());
        assert!(plan("CREATE TABLE t(a)").is_err());
        assert!(plan("CREATE INDEX aux.ix ON t(a)").is_err());
    }
}
