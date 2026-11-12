use crate::sync::Arc;
use crate::types::IOResultOr;
use rustc_hash::FxHashMap as HashMap;

use crate::alloc::TursoVecExt;
use crate::schema::Schema;
use crate::types::IOResult;
use crate::util::normalize_ident;
use crate::{Connection, Result, Statement, TransactionState, Value};
pub const STATS_TABLE: &str = "sqlite_stat1";
const STATS_QUERY: &str = "SELECT 0, tbl, idx, stat, NULL FROM sqlite_stat1";
/// `sqlite_stat1`'s rows and, where ANALYZE has kept them, its sketches and samples
/// (docs/design/planner.md, decision 20), told apart by their first column.
const STATS_AND_COLUMNS_QUERY: &str = "SELECT 0, tbl, idx, stat, NULL FROM sqlite_stat1 \
     UNION ALL SELECT 1, tbl, col, kind, data FROM __turso_internal_column_stats";

/// Statistics produced by ANALYZE for a single index.
#[derive(Clone, Debug, Default)]
pub struct IndexStat {
    /// Estimated total number of rows in the table/index when the stat was collected.
    pub total_rows: Option<u64>,
    /// Average number of rows per distinct key prefix, for each leftmost prefix
    /// of the index columns.
    ///
    /// These values come directly from sqlite_stat1's stat column (after the
    /// first number which is total_rows). For a stat string "1000 100 10 1":
    /// - total_rows = 1000
    /// - avg_rows_per_distinct_prefix = [100, 10, 1]
    ///
    /// Entry at position `i` means: on average, this many rows share the same
    /// values in the first `i + 1` columns of the index. Lower values indicate
    /// higher selectivity (more distinct prefixes).
    ///
    /// To compute number of distinct values (NDV) for prefix i:
    ///   ndv = total_rows / avg_rows_per_distinct_prefix[i]
    pub avg_rows_per_distinct_prefix: Vec<u64>,
}

/// A column's values as its sketch counted them (decision 20).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColumnStat {
    /// Its distinct non-NULL values, at least one where it has any, within the sketch's error.
    pub distinct: f64,
    pub non_null: u64,
    pub nulls: u64,
}

/// A table's sample (decision 20): at most [crate::column_stats::SAMPLE_ROWS] of its rows, each
/// the values of `columns`, the table's columns ANALYZE read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sample {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// For each of `columns`, the values the sample holds at least
    /// [crate::column_stats::SAMPLE_HITS] times, the most common first: each value's hash
    /// ([crate::column_stats::value_hash]) and its rows in the sample.
    pub common: Vec<Vec<(u64, usize)>>,
}

impl Sample {
    /// The sample of `rows`, each the values of `columns`, with each column's common values
    /// counted: a loop over the rows a column, at most [crate::column_stats::SAMPLE_ROWS] each.
    pub fn new(columns: Vec<String>, rows: Vec<Vec<Value>>) -> Self {
        let common = (0..columns.len())
            .map(|at| {
                let mut hits: HashMap<u64, usize> = HashMap::default();
                for value in rows.iter().filter_map(|row| row.get(at)) {
                    if let Some(h) = crate::column_stats::value_hash(value) {
                        let n = hits.entry(h).or_default();
                        *n = n.saturating_add(1);
                    }
                }
                let mut common: Vec<(u64, usize)> = hits
                    .into_iter()
                    .filter(|(_, n)| *n >= crate::column_stats::SAMPLE_HITS)
                    .collect();
                common.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                common
            })
            .collect();
        Self {
            columns,
            rows,
            common,
        }
    }
}

/// Statistics produced by ANALYZE for a single BTree table.
#[derive(Clone, Debug, Default)]
pub struct TableStat {
    /// Estimated row count for the table (sqlite_stat1 entry with a NULL index name).
    pub row_count: Option<u64>,
    /// Per-index statistics keyed by normalized index name.
    pub index_stats: HashMap<String, IndexStat>,
    /// Each column's sketched counts, keyed by its normalized name (decision 20).
    pub columns: HashMap<String, ColumnStat>,
    /// The table's sample; `None` where ANALYZE kept none, or its rows passed the sample's budget.
    pub sample: Option<Arc<Sample>>,
}

impl TableStat {
    /// Get or create the per-index statistics bucket for the given index name.
    pub fn index_stats_mut(&mut self, index_name: &str) -> &mut IndexStat {
        let index_name = normalize_ident(index_name);
        self.index_stats.entry(index_name).or_default()
    }
}

/// Container for ANALYZE statistics across the schema.
#[derive(Clone, Debug, Default)]
pub struct AnalyzeStats {
    /// Per-table statistics keyed by normalized table name.
    pub tables: HashMap<String, TableStat>,
    /// The bytes of the samples loaded, their records' (decision 20), charged against
    /// [crate::column_stats::LOADED_SAMPLE_BYTES].
    pub sample_bytes: usize,
}

impl AnalyzeStats {
    pub fn needs_refresh(&self) -> bool {
        self.tables.is_empty()
    }
    /// Get the statistics for a table, if present.
    pub fn table_stats(&self, table_name: &str) -> Option<&TableStat> {
        let table_name = normalize_ident(table_name);
        self.tables.get(&table_name)
    }

    /// Get or create the statistics bucket for a table.
    pub fn table_stats_mut(&mut self, table_name: &str) -> &mut TableStat {
        let table_name = normalize_ident(table_name);
        self.tables.entry(table_name).or_default()
    }

    /// Remove all statistics for a table.
    pub fn remove_table(&mut self, table_name: &str) {
        let table_name = normalize_ident(table_name);
        self.tables.remove(&table_name);
    }

    /// Remove statistics for a specific index on a table.
    pub fn remove_index(&mut self, table_name: &str, index_name: &str) {
        let table_name = normalize_ident(table_name);
        let index_name = normalize_ident(index_name);
        if let Some(table_stats) = self.tables.get_mut(&table_name) {
            table_stats.index_stats.remove(&index_name);
        }
    }
}

/// Best-effort refresh of the connection's in-memory ANALYZE stats.
///
/// Blocking form of [`refresh_analyze_stats_nonblock`]: the same state machine,
/// driven to completion here by pumping IO. That is only correct when whatever
/// the `sqlite_stat1` scan waits for can make progress without the caller's
/// help (real I/O, or another thread). A host that schedules connections
/// cooperatively on one thread must drive the non-blocking variant itself,
/// because an explicit yield is a request to run *other* connections, which
/// `io.step()` never does; see `Database::connect_async`.
pub fn refresh_analyze_stats(conn: &Arc<Connection>) {
    let io = conn.db.io.clone();
    let mut state = RefreshAnalyzeStatsState::default();
    loop {
        let pending = match refresh_analyze_stats_nonblock(conn, &mut state) {
            Ok(IOResult::Done(())) => return,
            Ok(IOResult::IO(pending)) => pending,
            Err(e) => {
                tracing::warn!("Failed to refresh analyze stats: {e}");
                return;
            }
        };
        // An explicit yield carries nothing to wait for; give the IO one turn,
        // as the blocking runner always did, and step the scan again.
        let advanced = if pending.is_explicit_yield() {
            io.step()
        } else {
            pending.wait(io.as_ref())
        };
        if let Err(e) = advanced {
            tracing::warn!("Failed to refresh analyze stats: {e}");
            return;
        }
    }
}

/// Carries the in-progress `sqlite_stat1` scan across IO yields for
/// [`refresh_analyze_stats_nonblock`].
#[derive(Default)]
pub enum RefreshAnalyzeStatsState {
    #[default]
    Start,
    Running {
        stmt: Box<Statement>,
        schema_snapshot: Arc<Schema>,
        stats: AnalyzeStats,
    },
}

/// Non-blocking variant of [`refresh_analyze_stats`]: best-effort refresh of
/// the connection's in-memory ANALYZE stats from `sqlite_stat1`, yielding IO
/// via the supplied state instead of pumping it. Errors are swallowed (matches
/// the blocking variant's best-effort contract).
pub fn refresh_analyze_stats_nonblock(
    conn: &Arc<Connection>,
    st: &mut RefreshAnalyzeStatsState,
) -> IOResultOr<()> {
    loop {
        match st {
            RefreshAnalyzeStatsState::Start => {
                if !conn.is_db_initialized() || conn.is_nested_stmt() {
                    return Ok(IOResult::Done(()));
                }
                if matches!(conn.get_tx_state(), TransactionState::Write { .. }) {
                    return Ok(IOResult::Done(()));
                }
                let schema_snapshot = { conn.schema.read().clone() };
                if schema_snapshot.get_btree_table(STATS_TABLE).is_none() {
                    return Ok(IOResult::Done(()));
                }
                let mv_tx = conn.get_mv_tx();
                let query = if schema_snapshot
                    .get_btree_table(crate::translate::analyze::COLUMN_STATS_TABLE)
                    .is_some()
                {
                    STATS_AND_COLUMNS_QUERY
                } else {
                    STATS_QUERY
                };
                let mut stmt = conn.prepare(query)?;
                stmt.set_mv_tx(mv_tx);
                *st = RefreshAnalyzeStatsState::Running {
                    stmt: Box::new(stmt),
                    schema_snapshot,
                    stats: AnalyzeStats::default(),
                };
            }
            RefreshAnalyzeStatsState::Running {
                stmt,
                schema_snapshot,
                stats,
            } => {
                let scan = load_sqlite_stat1_rows_nonblock(stmt, schema_snapshot, stats);
                match scan {
                    Ok(IOResult::IO(io)) => return Ok(IOResult::IO(io)),
                    Ok(IOResult::Done(())) => {
                        let stats = std::mem::take(stats);
                        if let Err(e) = install_analyze_stats(conn, stats) {
                            tracing::warn!("Failed to refresh analyze stats: {e}");
                        }
                        *st = RefreshAnalyzeStatsState::Start;
                        return Ok(IOResult::Done(()));
                    }
                    Err(e) => {
                        tracing::warn!("Failed to refresh analyze stats: {e}");
                        *st = RefreshAnalyzeStatsState::Start;
                        return Ok(IOResult::Done(()));
                    }
                }
            }
        }
    }
}

/// Store freshly gathered stats in the connection's schema and, when the shared
/// database schema is the same schema version, in the shared schema too.
///
/// In MVCC mode the commit that wrote `sqlite_stat1` publishes the connection's
/// schema to the shared schema before the stats are gathered. If only the
/// connection copy were updated, the two copies would differ while having the
/// same version, and the next statement would adopt the shared copy and lose
/// the new stats.
///
/// The connection schema lock is released before the shared schema lock is
/// taken: other code paths lock them in the opposite order.
fn install_analyze_stats(conn: &Arc<Connection>, stats: AnalyzeStats) -> Result<()> {
    let schema_version = conn.with_schema_mut(|schema| {
        schema.analyze_stats = stats.clone();
        schema.schema_version
    })?;
    let mut shared = conn.db.schema.lock();
    if shared.schema_version == schema_version {
        Schema::try_make_mut(&mut shared)?.analyze_stats = stats;
    }
    Ok(())
}

/// Non-blocking row scan shared by [`refresh_analyze_stats_nonblock`]. Steps the
/// prepared `sqlite_stat1` statement, accumulating into `stats`.
fn load_sqlite_stat1_rows_nonblock(
    stmt: &mut Statement,
    schema: &Schema,
    stats: &mut AnalyzeStats,
) -> crate::types::IOResultOr<()> {
    crate::return_if_io!(stmt.run_with_row_callback_nonblock(|row| {
        if row.get::<i64>(0)? == 1 {
            load_column_stats_row(row, schema, stats)
        } else {
            load_sqlite_stat1_row(row, schema, stats)
        }
    }));
    Ok(crate::types::IOResult::Done(()))
}

/// Apply a row of ANALYZE's sketches and samples (decision 20): `(1, tbl, col, 'sketch', its
/// encoding)` or `(1, tbl, NULL, 'sample', its encoding or NULL)`. A row of a table or column the
/// schema no longer holds, or that doesn't decode, is passed over: its estimates fall back to
/// `sqlite_stat1` and the fixed selectivities, as before ANALYZE kept it.
fn load_column_stats_row(
    row: &crate::vdbe::Row,
    schema: &Schema,
    stats: &mut AnalyzeStats,
) -> Result<()> {
    let table_name = row.get::<&str>(1)?;
    let Some(table) = schema.get_btree_table(table_name) else {
        return Ok(());
    };
    let (Value::Text(kind), Value::Blob(data)) = (row.get::<&Value>(3)?, row.get::<&Value>(4)?)
    else {
        return Ok(());
    };
    match kind.as_str() {
        "sketch" => {
            let Value::Text(column) = row.get::<&Value>(2)? else {
                return Ok(());
            };
            let Some(sketch) = crate::column_stats::Sketch::decode(data) else {
                return Ok(());
            };
            if table.get_column(column.as_str()).is_none() {
                return Ok(());
            }
            stats.table_stats_mut(table_name).columns.insert(
                normalize_ident(column.as_str()),
                ColumnStat {
                    distinct: sketch.distinct(),
                    non_null: sketch.non_null(),
                    nulls: sketch.nulls(),
                },
            );
        }
        "sample" => {
            // The samples loaded stay within their budget (CLAUDE.md §2): a table whose sample
            // would pass it keeps none, its estimates the distinct counts', and says so.
            let Some(charged) = stats
                .sample_bytes
                .checked_add(data.len())
                .filter(|n| *n <= crate::column_stats::LOADED_SAMPLE_BYTES)
            else {
                tracing::warn!(
                    target: "amphora.planner.stats",
                    event = "sample_over_budget",
                    table = table_name,
                    bytes = data.len(),
                    "a table's sample is not loaded: the samples loaded would pass their budget"
                );
                return Ok(());
            };
            let Some(records) = crate::column_stats::decode_sample(data) else {
                return Ok(());
            };
            // The columns ANALYZE reads, as `emit_column_stats` chooses them.
            let columns: Vec<String> = table
                .columns()
                .iter()
                .filter(|c| !c.is_generated())
                .filter_map(|c| c.name.as_deref().map(normalize_ident))
                .collect();
            let mut rows = Vec::with_capacity(records.len());
            for record in records {
                let values =
                    crate::types::ImmutableRecordRef::from_bin_record(record).get_values_owned()?;
                // A record of another shape was kept before the table's columns changed.
                if values.len() != columns.len() {
                    return Ok(());
                }
                rows.push(values);
            }
            stats.sample_bytes = charged;
            stats.table_stats_mut(table_name).sample = Some(Arc::new(Sample::new(columns, rows)));
        }
        _ => {}
    }
    Ok(())
}

/// Apply a single `sqlite_stat1` row to the accumulating [`AnalyzeStats`].
/// Shared by the blocking and non-blocking scanners.
fn load_sqlite_stat1_row(
    row: &crate::vdbe::Row,
    schema: &Schema,
    stats: &mut AnalyzeStats,
) -> Result<()> {
    let table_name = row.get::<&str>(1)?;
    let idx_value = row.get::<&Value>(2)?;
    let stat_value = row.get::<&Value>(3)?;

    let idx_name = match idx_value {
        Value::Null => None,
        Value::Text(s) => Some(s.as_str()),
        _ => None,
    };
    let stat = match stat_value {
        Value::Text(s) => s.as_str(),
        _ => return Ok(()),
    };

    // Skip if table is not a regular B-tree.
    if schema.get_btree_table(table_name).is_none() {
        return Ok(());
    }
    let Some(numbers) = parse_stat_numbers(stat) else {
        return Ok(());
    };
    if numbers.is_empty() {
        return Ok(());
    }
    if idx_name.is_none() {
        if let Some(total_rows) = numbers.first().copied() {
            stats.table_stats_mut(table_name).row_count = Some(total_rows);
        }
        return Ok(());
    }

    // Index-level entry: only keep if the index exists on this table, or names the table itself: a
    // WITHOUT ROWID table's primary key, as SQLite's analysisLoader takes it ("sqlite3_stricmp(argv[0],
    // argv[1])==0"), which the planner finds by its descriptor's name, the table's (bug 5adc6889).
    let idx_name = normalize_ident(idx_name.unwrap());
    let names_primary_key = idx_name.eq_ignore_ascii_case(table_name)
        && schema
            .get_btree_table(table_name)
            .is_some_and(|table| !table.has_rowid);
    if !names_primary_key && schema.get_index(table_name, &idx_name).is_none() {
        return Ok(());
    }

    let total_rows = numbers.first().copied();
    {
        let idx_stats = stats.table_stats_mut(table_name).index_stats_mut(&idx_name);
        idx_stats.total_rows = total_rows;
        idx_stats.avg_rows_per_distinct_prefix = numbers.iter().skip(1).copied().collect();
    }

    // If we didn't see a table-level row yet, seed row_count from index stats.
    if let Some(total_rows) = total_rows {
        let table_stats = stats.table_stats_mut(table_name);
        if table_stats.row_count.is_none() {
            table_stats.row_count = Some(total_rows);
        }
    }
    Ok(())
}

fn parse_stat_numbers(stat: &str) -> Option<Vec<u64>> {
    stat.split_whitespace()
        .map(|s| s.parse::<u64>().ok())
        .collect()
}

/// Statistics accumulator for ANALYZE.
#[derive(Debug, Clone)]
pub struct StatAccum {
    /// Number of columns in the index (not including rowid)
    pub n_col: usize,
    /// Total number of rows seen
    pub n_row: u64,
    /// Distinct counts for each column prefix.
    /// distinct[i] = number of distinct values for columns 0..=i
    pub distinct: Vec<u64>,
}

impl StatAccum {
    pub fn new(n_col: usize) -> Self {
        Self {
            n_col,
            n_row: 0,
            distinct: vec![0; n_col],
        }
    }

    /// Push a row, indicating which column (0-indexed) is the first to differ
    /// from the previous row. If this is the first row, pass 0.
    ///
    /// i_chng is the index of the leftmost column that changed:
    /// - 0 means column 0 changed (or first row)
    /// - 1 means columns 0 was same, column 1 changed
    /// - n_col means all columns were the same (duplicate row)
    pub fn push(&mut self, i_chng: usize) {
        self.n_row += 1;
        // Increment distinct counts for columns i_chng and onwards
        // because if column i changed, then prefixes (0..=i), (0..=i+1), etc. all have a new distinct value
        for i in i_chng..self.n_col {
            self.distinct[i] += 1;
        }
    }

    /// Get the stat1 string: "total avg1 avg2 ..."
    /// where avgN = ceil(total / distinctN)
    pub fn get_stat1(&self) -> String {
        if self.n_row == 0 {
            return String::new();
        }

        let mut parts = vec![self.n_row.to_string()];
        for &d in &self.distinct {
            let avg = if d > 0 {
                self.n_row.div_ceil(d)
            } else {
                self.n_row
            };
            parts.push(avg.to_string());
        }
        parts.join(" ")
    }

    /// Serialize to bytes for storage in a blob register.
    pub fn to_bytes(&self) -> crate::ValueBlob {
        let mut bytes =
            <crate::ValueBlob as TursoVecExt<u8>>::with_capacity(8 + 8 + 8 * self.n_col);
        bytes.extend_from_slice(&(self.n_col as u64).to_le_bytes());
        bytes.extend_from_slice(&self.n_row.to_le_bytes());
        for &d in &self.distinct {
            bytes.extend_from_slice(&d.to_le_bytes());
        }
        bytes
    }

    /// Deserialize from bytes.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 16 {
            return None;
        }
        let n_col = u64::from_le_bytes(bytes[0..8].try_into().ok()?) as usize;
        let n_row = u64::from_le_bytes(bytes[8..16].try_into().ok()?);

        if bytes.len() < 16 + 8 * n_col {
            return None;
        }
        let mut distinct = Vec::with_capacity(n_col);
        for i in 0..n_col {
            let start = 16 + i * 8;
            let d = u64::from_le_bytes(bytes[start..start + 8].try_into().ok()?);
            distinct.push(d);
        }
        Some(Self {
            n_col,
            n_row,
            distinct,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::parse_stat_numbers;

    #[test]
    fn parse_stat_numbers_basic() {
        assert_eq!(parse_stat_numbers("10 5 3 1").unwrap(), vec![10, 5, 3, 1]);
        assert_eq!(parse_stat_numbers("  42\t7 ").unwrap(), vec![42, 7]);
        assert!(parse_stat_numbers("abc 1").is_none());
    }
}
