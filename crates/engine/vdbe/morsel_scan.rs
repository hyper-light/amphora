//! Morsels over a table's own pages (QT.6, the query-speed plan's step 6; G5 step 3): each worker
//! opens its own cursor over a run of the table b-tree's leaf pages, in the statement's one
//! snapshot (every cursor reads through the statement's pager), and runs the pipeline's loop body
//! (`morsel.rs`: the scan's filter and aggregates, or a hash join's probe) over the rows it finds.
//! The partials merge as `morsel.rs`'s do. docs/design/executor.md section 8 states the design.
//!
//! A morsel is a run of leaf pages, so the unit `choose_degree` sizes is a page: the pilot measures
//! a page's cost, and the model picks the workers and the pages per morsel. The leaves are found
//! by reading the table's interior pages level by level (no recursion; a level's pages in key
//! order), keeping each leaf's greatest rowid from its parent's divider. A worker's cursor seeks
//! past the divider below its first leaf and reads until a rowid passes the divider above its last.
//! A hash join's probe with its aggregates (`TableJoinAggregate`) builds its table once, the
//! workers together (`TableBuild`: each scans its own build leaves and scatters its rows' entries
//! by partition, then each partition is filled by one worker), then each worker probes over its
//! own leaves and runs each joined row through the copy of the serial loop's body.

use std::ops::Range;

use super::morsel::{
    execute, execute_at, key_part, Degree, HashProbe, KeyPart, MachineCosts, MemoryBudget,
    MorselError, ParallelMode, PartitionedTable, Partitioner, Pipeline, Predicate, RunStats,
    ScanAggregate, Scattered,
};
use crate::storage::btree::{BTreeCursor, CursorTrait};
use crate::storage::pager::{PageRef, Pager};
use crate::storage::sqlite3_ondisk::PageType;
use crate::sync::Arc;
use crate::types::{IOResult, SeekKey, SeekOp, SeekResult, Value};
use crate::util::IOExt;
use crate::vdbe::insn::MorselOuter;

fn storage(error: crate::LimboError) -> MorselError {
    MorselError::Storage(Box::new(error))
}

/// A table b-tree read in morsels of leaf pages, through the statement's pager.
pub struct TableSource {
    pager: Arc<Pager>,
    root: i64,
    columns: usize,
    rowid_alias: Option<usize>,
    /// Each column's value in a row written before it was added; empty when every one is NULL.
    defaults: Vec<Value>,
    /// Each leaf's greatest rowid in key order; `None` for the last leaf, which has no bound.
    upper: Vec<Option<i64>>,
    /// The greatest rowid before the first leaf `upper` holds; `None` when that is the table's
    /// first (lane qs6-17: [`TableSource::restrict`]).
    floor: Option<i64>,
}

/// One page entry of a level of the walk: its number, the greatest rowid under it, and whether it
/// is known to be a leaf.
type LevelPage = (i64, Option<i64>, bool);

impl TableSource {
    /// Finds the leaves of the table b-tree rooted at `root` (a table of `columns` columns, the
    /// rowid standing in for column `rowid_alias` when it is an INTEGER PRIMARY KEY, a short row's
    /// missing columns read as `defaults` has them, NULL past its end).
    pub fn open(
        pager: Arc<Pager>,
        root: i64,
        columns: usize,
        rowid_alias: Option<usize>,
        defaults: Vec<Value>,
    ) -> Result<Self, MorselError> {
        // The bound is the format's: no b-tree has more pages than the database may hold.
        let most_pages =
            usize::try_from(pager.get_max_page_count()).map_err(|_| MorselError::Overflow {
                what: "the database's page count",
            })?;
        let mut level: Vec<LevelPage> = Vec::new();
        level.try_reserve_exact(1)?;
        level.push((root, None, false));
        let mut visited = 1usize;
        // Ends: each pass replaces every interior page with its children, one level deeper; a
        // tree has fewer levels than pages, and `visited` refuses a walk past the page count.
        loop {
            if level.iter().all(|(_, _, leaf)| *leaf) {
                break;
            }
            let mut next: Vec<LevelPage> = Vec::new();
            for (number, upper, leaf) in level {
                if leaf {
                    next.try_reserve(1)?;
                    next.push((number, upper, true));
                    continue;
                }
                let page = read_page(&pager, number)?;
                let contents = page.get_contents();
                match contents.page_type().map_err(storage)? {
                    PageType::TableLeaf => {
                        next.try_reserve(1)?;
                        next.push((number, upper, true));
                    }
                    PageType::TableInterior => {
                        let cells = contents.cell_count();
                        next.try_reserve(cells.saturating_add(1))?;
                        for cell in 0..cells {
                            let child = contents
                                .cell_interior_read_left_child_page(cell)
                                .map_err(storage)?;
                            let divider = contents
                                .cell_table_interior_read_rowid(cell)
                                .map_err(storage)?;
                            next.push((i64::from(child), Some(divider), false));
                        }
                        let right = contents
                            .rightmost_pointer()
                            .map_err(storage)?
                            .ok_or(MorselError::Corrupt("an interior page has no right child"))?;
                        next.push((i64::from(right), upper, false));
                        visited = visited.saturating_add(cells.saturating_add(1));
                        if visited > most_pages {
                            return Err(MorselError::Corrupt(
                                "the walk passed the database's page count",
                            ));
                        }
                    }
                    PageType::IndexInterior | PageType::IndexLeaf => {
                        return Err(MorselError::Corrupt("a table b-tree holds an index page"));
                    }
                }
            }
            level = next;
        }
        let mut upper = Vec::new();
        upper.try_reserve_exact(level.len())?;
        upper.extend(level.into_iter().map(|(_, bound, _)| bound));
        Ok(TableSource {
            pager,
            root,
            columns,
            rowid_alias,
            defaults,
            upper,
            floor: None,
        })
    }

    /// Fills a short row out to the table's columns: a column added after the row was written
    /// reads as its default, as the serial loop's Column reads it (NULL when it has none; lane
    /// qs6-17).
    fn pad(&self, row: &mut Vec<Value>) {
        let written = row.len();
        row.resize(self.columns, Value::Null);
        for (slot, default) in row.iter_mut().zip(&self.defaults).skip(written) {
            slot.clone_from(default);
        }
    }

    /// Keeps only the leaves that may hold a rowid from `lower` to `upper` (each inclusive, `None`
    /// unbounded): the first whose greatest rowid reaches `lower`, through the first whose greatest
    /// rowid reaches `upper` (lane qs6-17). The rows past the bounds in those leaves are still
    /// read: the caller's terms decide them.
    pub fn restrict(&mut self, lower: Option<i64>, upper: Option<i64>) {
        // A leaf's bound is `None` only for the last, which holds every rowid past the others'.
        let reaches = |bound: &Option<i64>, rowid: i64| bound.is_none_or(|b| b >= rowid);
        let first = lower.map_or(0, |lower| {
            self.upper.partition_point(|bound| !reaches(bound, lower))
        });
        let end = upper.map_or(self.upper.len(), |upper| {
            self.upper
                .partition_point(|bound| !reaches(bound, upper))
                .saturating_add(1)
                .min(self.upper.len())
        });
        let end = end.max(first);
        if let Some(below) = first.checked_sub(1) {
            self.floor = self.upper.get(below).copied().flatten();
        }
        self.upper.truncate(end);
        self.upper.drain(..first);
    }

    /// How many leaf pages the table has: the units its morsels are cut in.
    pub fn leaves(&self) -> usize {
        self.upper.len()
    }

    /// Reads the rows of leaves `leaves` in rowid order with a cursor of its own, handing each
    /// row (its rowid, then its columns) to `each`.
    pub fn scan(
        &self,
        leaves: Range<usize>,
        mut each: impl FnMut(i64, &[Value]) -> Result<(), MorselError>,
    ) -> Result<(), MorselError> {
        self.scan_while(leaves, |rowid, row| each(rowid, row).map(|()| true))
    }

    /// As [`TableSource::scan`], stopping after the row for which `each` says false.
    pub fn scan_while(
        &self,
        leaves: Range<usize>,
        mut each: impl FnMut(i64, &[Value]) -> Result<bool, MorselError>,
    ) -> Result<(), MorselError> {
        if leaves.start >= leaves.end {
            return Ok(());
        }
        let lower = match leaves.start.checked_sub(1) {
            None => self.floor,
            Some(below) => Some(self.upper.get(below).copied().flatten().ok_or(
                MorselError::Corrupt("a leaf before the last has no greatest rowid"),
            )?),
        };
        let last = leaves.end.checked_sub(1).ok_or(MorselError::Overflow {
            what: "a morsel's last leaf",
        })?;
        let upper = *self.upper.get(last).ok_or(MorselError::Overflow {
            what: "a morsel's leaves",
        })?;
        let io = &self.pager.io;
        let mut cursor = BTreeCursor::new_table(self.pager.clone(), self.root, self.columns);
        let positioned = match lower {
            None => {
                io.block(|| cursor.rewind()).map_err(storage)?;
                true
            }
            Some(lower) => {
                match io
                    .block(|| cursor.seek(SeekKey::TableRowId(lower), SeekOp::GT))
                    .map_err(storage)?
                {
                    SeekResult::Found => true,
                    SeekResult::NotFound => false,
                    SeekResult::TryAdvance => {
                        io.block(|| cursor.next()).map_err(storage)?;
                        true
                    }
                }
            }
        };
        if !positioned {
            return Ok(());
        }
        let mut row: Vec<Value> = Vec::new();
        row.try_reserve_exact(self.columns)?;
        // Ends: each pass moves the cursor to the next row; the loop stops when the rows run out
        // or a rowid passes the morsel's last leaf.
        while let Some(rowid) = io.block(|| cursor.rowid()).map_err(storage)? {
            if upper.is_some_and(|bound| rowid > bound) {
                break;
            }
            let values = io
                .block(|| {
                    Ok(match cursor.record()? {
                        IOResult::Done(Some(record)) => {
                            IOResult::Done(Some(record.get_values_owned()?))
                        }
                        IOResult::Done(None) => IOResult::Done(None),
                        IOResult::IO(wait) => IOResult::IO(wait),
                    })
                })
                .map_err(storage)?
                .ok_or(MorselError::Corrupt("a row has no record"))?;
            row.clear();
            row.extend(values.into_iter().take(self.columns));
            self.pad(&mut row);
            if let Some(at) = self.rowid_alias {
                let slot = row.get_mut(at).ok_or(MorselError::NoColumn {
                    column: at,
                    width: self.columns,
                })?;
                *slot = Value::from_i64(rowid);
            }
            if !each(rowid, &row)? {
                break;
            }
            io.block(|| cursor.next()).map_err(storage)?;
        }
        Ok(())
    }

    /// A cursor of the caller's own on the table, for [`TableSource::seek_row`] (lane qs6-16).
    pub fn cursor(&self) -> BTreeCursor {
        BTreeCursor::new_table(self.pager.clone(), self.root, self.columns)
    }

    /// Reads the row `rowid` into `row` through `cursor` (one of [`TableSource::cursor`]'s), its
    /// columns as [`TableSource::scan`] gives them; false when the table has no such row.
    pub fn seek_row(
        &self,
        cursor: &mut BTreeCursor,
        rowid: i64,
        row: &mut Vec<Value>,
    ) -> Result<bool, MorselError> {
        let io = &self.pager.io;
        let found = io
            .block(|| cursor.seek(SeekKey::TableRowId(rowid), SeekOp::GE { eq_only: true }))
            .map_err(storage)?;
        if !matches!(found, SeekResult::Found) {
            return Ok(false);
        }
        let values = io
            .block(|| {
                Ok(match cursor.record()? {
                    IOResult::Done(Some(record)) => {
                        IOResult::Done(Some(record.get_values_owned()?))
                    }
                    IOResult::Done(None) => IOResult::Done(None),
                    IOResult::IO(wait) => IOResult::IO(wait),
                })
            })
            .map_err(storage)?
            .ok_or(MorselError::Corrupt("a row has no record"))?;
        row.clear();
        row.try_reserve(self.columns)?;
        row.extend(values.into_iter().take(self.columns));
        self.pad(&mut row);
        if let Some(at) = self.rowid_alias {
            let slot = row.get_mut(at).ok_or(MorselError::NoColumn {
                column: at,
                width: self.columns,
            })?;
            *slot = Value::from_i64(rowid);
        }
        Ok(true)
    }

    /// Every row in rowid order, read on the caller's thread: a hash join's build side.
    pub fn rows(&self) -> Result<Vec<(i64, Vec<Value>)>, MorselError> {
        let mut out = Vec::new();
        self.scan(0..self.leaves(), |rowid, row| {
            out.try_reserve(1)?;
            let mut owned = Vec::new();
            owned.try_reserve_exact(row.len())?;
            owned.extend_from_slice(row);
            out.push((rowid, owned));
            Ok(())
        })?;
        Ok(out)
    }
}

fn read_page(pager: &Pager, number: i64) -> Result<PageRef, MorselError> {
    let (page, completion) = pager
        .io
        .block(|| pager.read_page(number))
        .map_err(storage)?;
    if let Some(completion) = completion {
        pager.io.wait_for_completion(completion).map_err(storage)?;
    }
    Ok(page)
}

/// A rowid's place in rowid order as a row position (the order MIN, MAX and a group's key values
/// break ties by): the sign bit flipped, so every rowid maps in order onto 0..2^64.
fn position(rowid: i64) -> Result<usize, MorselError> {
    let ordered = u64::from_ne_bytes(rowid.to_ne_bytes()) ^ (1u64 << 63);
    usize::try_from(ordered).map_err(|_| MorselError::Overflow {
        what: "a rowid's position",
    })
}

/// A scan with its filter and aggregates over a table's leaves: `shape` names the filter, the
/// grouping and the aggregates (its `rows` are not read).
pub struct TableAggregate<'a> {
    pub table: &'a TableSource,
    pub shape: &'a ScanAggregate<'a>,
}

impl Pipeline for TableAggregate<'_> {
    type Partial = super::morsel::AggregatePartial;

    fn rows(&self) -> usize {
        self.table.leaves()
    }

    fn empty(&self) -> Self::Partial {
        self.shape.empty()
    }

    fn run(
        &self,
        leaves: Range<usize>,
        partial: &mut Self::Partial,
        budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        self.table.scan(leaves, |rowid, row| {
            self.shape.fold(row, position(rowid)?, partial, budget)
        })
    }

    fn merge(
        &self,
        into: &mut Self::Partial,
        from: Self::Partial,
        budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        self.shape.merge(into, from, budget)
    }
}

impl TableAggregate<'_> {
    /// The answer as the switch and the measured costs pick the degree.
    pub fn run_mode(
        &self,
        mode: ParallelMode,
        costs: &mut MachineCosts,
        budget: &MemoryBudget,
    ) -> Result<(Vec<Vec<Value>>, RunStats), MorselError> {
        let (partial, stats) = execute(self, mode, costs, budget)?;
        Ok((self.shape.finish(partial)?, stats))
    }

    /// The answer at a stated degree, its morsels counted in leaf pages.
    pub fn run_at(
        &self,
        degree: Degree,
        budget: &MemoryBudget,
    ) -> Result<(Vec<Vec<Value>>, RunStats), MorselError> {
        let (partial, stats) = execute_at(self, 0, degree, None, budget)?;
        Ok((self.shape.finish(partial)?, stats))
    }
}

/// A one-table scan whose filter, GROUP BY values and aggregate inputs are a copy of the serial
/// loop's body (`MorselBody`, lane qs6-7): each worker runs that range of the statement's program
/// per row, with registers and cursor of its own, and folds the rows it keeps into its partial
/// aggregates. Any expression the serial loop runs runs here, through the VDBE's own opcodes.
pub struct TableBodyAggregate<'a> {
    pub table: &'a TableSource,
    pub body: &'a crate::vdbe::insn::MorselBody,
    pub program: &'a crate::vdbe::Program,
    pub pager: &'a Arc<Pager>,
    /// The statement's non-NULL registers when the hook ran, by register: each worker's start.
    pub registers: &'a [(usize, Value)],
    pub parameters: &'a [Value],
    pub shape: &'a ScanAggregate<'a>,
    /// Counts the rows the copied body ran over.
    pub ran: &'a std::sync::atomic::AtomicU64,
    /// The body's direct reads when the connection's switch takes them (lane qs6-13,
    /// T-QT.6.21): a joined row's outputs copied from it, no code run.
    pub direct: Option<&'a [Option<usize>]>,
}

/// A worker's own state for [`TableBodyAggregate`]: its registers, cursors and program counter,
/// made at its first morsel, and the output row it reuses.
pub struct BodyWorker {
    state: crate::vdbe::ProgramState,
    out: Vec<Value>,
}

/// The partial of [`TableBodyAggregate`]: the aggregates, and the worker that folds into them.
#[derive(Default)]
pub struct BodyPartial {
    aggregate: super::morsel::AggregatePartial,
    worker: Option<Box<BodyWorker>>,
}

impl TableBodyAggregate<'_> {
    fn worker(&self) -> Result<BodyWorker, MorselError> {
        let mut state = crate::vdbe::ProgramState::new(
            self.program.max_registers,
            self.program.cursor_ref.len(),
        );
        for (at, value) in self.registers {
            let register = state
                .registers
                .get_mut(*at)
                .ok_or(MorselError::Corrupt("a register past the program's"))?;
            *register = crate::vdbe::Register::Value(value.clone());
        }
        state.parameters.try_reserve_exact(self.parameters.len())?;
        state.parameters.extend(self.parameters.iter().cloned());
        let mut out = Vec::new();
        out.try_reserve_exact(self.body.out_count)?;
        Ok(BodyWorker { state, out })
    }

    /// Runs the body over one row; true when the row passed, its values then in `worker.out`.
    fn row(&self, worker: &mut BodyWorker, rowid: i64, row: &[Value]) -> Result<bool, MorselError> {
        if let Some(direct) = self.direct {
            // Nothing but reads (lane qs6-13): the values the body's copy would read, every row
            // passing, a REAL where `no_real` names still sending the statement back.
            worker.out.clear();
            for at in direct {
                worker.out.push(match at {
                    Some(at) => row
                        .get(*at)
                        .ok_or(MorselError::NoColumn {
                            column: *at,
                            width: row.len(),
                        })?
                        .clone(),
                    None => Value::from_i64(rowid),
                });
            }
            self.no_real(worker)?;
            return Ok(true);
        }
        self.load(worker, Some(rowid), row)?;
        self.passes(worker, self.body.start, self.body.keep)
    }

    /// A join's joined row (lane qs6-13): whether it matched and whether it passed. An inner
    /// join's always matched; with `pad_last` (a LEFT join's last link) a padded row did not and
    /// is tested by the WHERE terms alone, any other by the ON terms first.
    fn joined_row(
        &self,
        worker: &mut BodyWorker,
        rowid: i64,
        row: &[Value],
        pad_last: Option<usize>,
        padded: bool,
    ) -> Result<(bool, bool), MorselError> {
        match (pad_last, padded) {
            (None, _) => Ok((true, self.row(worker, rowid, row)?)),
            (Some(_), true) => Ok((false, self.padded_row(worker, Some(rowid), row)?)),
            (Some(_), false) => self.on_row(worker, Some(rowid), row),
        }
    }

    /// An outer join's joined row (lane qs6-12): whether it passed the ON terms (it matched), and
    /// whether it then passed the rest of the body, its values then in `worker.out`.
    fn on_row(
        &self,
        worker: &mut BodyWorker,
        rowid: Option<i64>,
        row: &[Value],
    ) -> Result<(bool, bool), MorselError> {
        self.load(worker, rowid, row)?;
        let on = self.body.on.unwrap_or(self.body.start);
        if !self.run_to(worker, on, self.body.start)? {
            return Ok((false, false));
        }
        Ok((true, self.passes(worker, self.body.start, self.body.keep)?))
    }

    /// A row an outer join pads with NULLs (lane qs6-12): its ON terms are not tested, only the
    /// WHERE terms and the outputs; `rowid` is none when the probe's side is the NULLs.
    fn padded_row(
        &self,
        worker: &mut BodyWorker,
        rowid: Option<i64>,
        row: &[Value],
    ) -> Result<bool, MorselError> {
        self.load(worker, rowid, row)?;
        self.passes(worker, self.body.start, self.body.keep)
    }

    /// Writes a row's columns and its rowid (NULL when there is none) into the body's registers.
    fn load(
        &self,
        worker: &mut BodyWorker,
        rowid: Option<i64>,
        row: &[Value],
    ) -> Result<(), MorselError> {
        use crate::vdbe::Register;
        let state = &mut worker.state;
        for (at, value) in row.iter().enumerate() {
            let reg = self
                .body
                .columns_start
                .checked_add(at)
                .ok_or(MorselError::Overflow {
                    what: "a column's register",
                })?;
            let register = state.registers.get_mut(reg).ok_or(MorselError::Corrupt(
                "the loop body's register is past the program's",
            ))?;
            *register = Register::Value(value.clone());
        }
        *state
            .registers
            .get_mut(self.body.rowid)
            .ok_or(MorselError::Corrupt(
                "the loop body's register is past the program's",
            ))? = Register::Value(rowid.map_or(Value::Null, Value::from_i64));
        Ok(())
    }

    /// Runs the body's code from `from` until it reaches `pass` (true) or `skip` (false), then,
    /// when it passed, reads its outputs into `worker.out`.
    fn passes(
        &self,
        worker: &mut BodyWorker,
        from: crate::vdbe::BranchOffset,
        pass: crate::vdbe::BranchOffset,
    ) -> Result<bool, MorselError> {
        if !self.run_to(worker, from, pass)? {
            return Ok(false);
        }
        self.outputs(worker)?;
        Ok(true)
    }

    /// Runs the body's code from `from` until it reaches `pass` (true) or `skip` (false).
    fn run_to(
        &self,
        worker: &mut BodyWorker,
        from: crate::vdbe::BranchOffset,
        pass: crate::vdbe::BranchOffset,
    ) -> Result<bool, MorselError> {
        use crate::vdbe::execute::InsnFunctionStepResult;
        let state = &mut worker.state;
        let pass = pass.as_offset_int();
        let skip = self.body.skip.as_offset_int();
        state.pc = from.as_offset_int();
        // The body's code jumps forward only (a filter's branches, CASE, a constant call's
        // `Once`), so it reaches `pass` or `skip` within its own length.
        loop {
            if state.pc == pass {
                return Ok(true);
            }
            if state.pc == skip {
                return Ok(false);
            }
            let at = usize::try_from(state.pc).map_err(|_| MorselError::Overflow {
                what: "the program counter",
            })?;
            let (insn, _) = self
                .program
                .insns
                .get(at)
                .ok_or(MorselError::Corrupt("the loop body ran past the program"))?;
            match insn.to_function()(self.program, state, insn, self.pager) {
                Ok(InsnFunctionStepResult::Step) => {}
                Ok(_) => {
                    return Err(MorselError::Body(Box::new(
                        crate::LimboError::InternalError(
                            "the loop body stopped short of its row".into(),
                        ),
                    )))
                }
                Err(error) => return Err(MorselError::Body(error)),
            }
        }
    }

    /// Reads the body's outputs into `worker.out`; a REAL where `no_real` names sends the
    /// statement to the serial loop.
    fn outputs(&self, worker: &mut BodyWorker) -> Result<(), MorselError> {
        use crate::vdbe::Register;
        worker.out.clear();
        let end =
            self.body
                .out_start
                .checked_add(self.body.out_count)
                .ok_or(MorselError::Overflow {
                    what: "the output registers",
                })?;
        let values =
            worker
                .state
                .registers
                .get(self.body.out_start..end)
                .ok_or(MorselError::Corrupt(
                    "the loop body's output is past the program's registers",
                ))?;
        for register in values {
            let Register::Value(value) = register else {
                return Err(MorselError::Corrupt(
                    "the loop body left a non-value output",
                ));
            };
            worker.out.push(value.clone());
        }
        self.no_real(worker)
    }

    /// A REAL where `no_real` names sends the statement to the serial loop.
    fn no_real(&self, worker: &BodyWorker) -> Result<(), MorselError> {
        for at in &self.body.no_real {
            if let Some(Value::Numeric(crate::numeric::Numeric::Float(_))) = worker.out.get(*at) {
                return Err(MorselError::Unsupported { kind: "REAL" });
            }
        }
        Ok(())
    }

    /// The answer as the switch and the measured costs pick the degree.
    pub fn run_mode(
        &self,
        mode: ParallelMode,
        costs: &mut MachineCosts,
        budget: &MemoryBudget,
    ) -> Result<(Vec<Vec<Value>>, RunStats), MorselError> {
        let (partial, stats) = execute(self, mode, costs, budget)?;
        Ok((self.shape.finish(partial.aggregate)?, stats))
    }
}

impl Pipeline for TableBodyAggregate<'_> {
    type Partial = BodyPartial;

    fn rows(&self) -> usize {
        self.table.leaves()
    }

    fn empty(&self) -> Self::Partial {
        BodyPartial::default()
    }

    fn run(
        &self,
        leaves: Range<usize>,
        partial: &mut Self::Partial,
        budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        let BodyPartial { aggregate, worker } = partial;
        let worker = match worker {
            Some(worker) => worker,
            None => worker.insert(Box::new(self.worker()?)),
        };
        let mut ran = 0u64;
        let scanned = self.table.scan(leaves, |rowid, row| {
            ran = ran.saturating_add(1);
            if !self.row(worker, rowid, row)? {
                return Ok(());
            }
            self.shape
                .fold(&worker.out, position(rowid)?, aggregate, budget)
        });
        if scanned.is_ok() {
            self.ran
                .fetch_add(ran, std::sync::atomic::Ordering::Relaxed);
        }
        scanned
    }

    fn merge(
        &self,
        into: &mut Self::Partial,
        from: Self::Partial,
        budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        self.shape
            .merge(&mut into.aggregate, from.aggregate, budget)
    }
}

/// A one-table scan's rows through the copy of the serial loop's body (lane qs6-8): each worker
/// runs the body per row as [`TableBodyAggregate`] does and keeps the outputs of the rows that
/// pass; the runs are joined in leaf order, so the rows come out in the serial loop's order. Under
/// a LIMIT (`want`) no morsel is claimed once that many rows are kept, and a morsel stops once it
/// alone holds that many. Under an ORDER BY (lane qs6-17) each worker sorts its morsel's rows and
/// the runs, in leaf order, are merged by a stable merge, as [`TableJoinRows`]' are: rows equal
/// under the keys keep their rowid order; no LIMIT stops it early.
pub struct TableBodyRows<'a> {
    pub body: &'a TableBodyAggregate<'a>,
    pub want: Option<usize>,
    /// The rows kept so far, by every worker.
    pub kept: std::sync::atomic::AtomicUsize,
    /// The sort keys: each one's place in the output row, and whether it descends.
    pub order: &'a [(usize, bool)],
}

/// The partial of [`TableBodyRows`]: each morsel's rows tagged with its first leaf, and the
/// worker that runs the body.
#[derive(Default)]
pub struct BodyRowsPartial {
    runs: Vec<(usize, Vec<Vec<Value>>)>,
    worker: Option<Box<BodyWorker>>,
}

impl TableBodyRows<'_> {
    /// The rows in leaf order, the first `want` of them under a LIMIT.
    pub fn finish(&self, mut partial: BodyRowsPartial) -> Result<Vec<Vec<Value>>, MorselError> {
        partial.runs.sort_by_key(|(first, _)| *first);
        let total = partial
            .runs
            .iter()
            .map(|(_, run)| run.len())
            .fold(0usize, usize::saturating_add);
        let take = self.want.map_or(total, |want| want.min(total));
        let mut out = Vec::new();
        out.try_reserve_exact(take)?;
        for (_, run) in partial.runs {
            let room = take.saturating_sub(out.len());
            out.extend(run.into_iter().take(room));
        }
        // `sort_by` is a stable merge sort that finds the workers' sorted runs and merges them.
        if !self.order.is_empty() {
            out.sort_by(|a, b| compare_rows(self.order, a, b));
        }
        Ok(out)
    }

    /// The rows as the switch and the measured costs pick the degree.
    pub fn run_mode(
        &self,
        mode: ParallelMode,
        costs: &mut MachineCosts,
        budget: &MemoryBudget,
    ) -> Result<(Vec<Vec<Value>>, RunStats), MorselError> {
        let (partial, stats) = execute(self, mode, costs, budget)?;
        Ok((self.finish(partial)?, stats))
    }
}

impl Pipeline for TableBodyRows<'_> {
    type Partial = BodyRowsPartial;

    fn rows(&self) -> usize {
        self.body.table.leaves()
    }

    fn empty(&self) -> Self::Partial {
        BodyRowsPartial::default()
    }

    fn done(&self) -> bool {
        self.want
            .is_some_and(|want| self.kept.load(std::sync::atomic::Ordering::Relaxed) >= want)
    }

    fn run(
        &self,
        leaves: Range<usize>,
        partial: &mut Self::Partial,
        budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        let BodyRowsPartial { runs, worker } = partial;
        let worker = match worker {
            Some(worker) => worker,
            None => worker.insert(Box::new(self.body.worker()?)),
        };
        let first = leaves.start;
        let mut found: Vec<Vec<Value>> = Vec::new();
        let mut ran = 0u64;
        let scanned = self.body.table.scan_while(leaves, |rowid, row| {
            ran = ran.saturating_add(1);
            if !self.body.row(worker, rowid, row)? {
                return Ok(true);
            }
            found.try_reserve(1)?;
            found.push(kept(&worker.out, budget)?);
            self.kept.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // This morsel alone holds the rows the LIMIT takes: none after them is among those.
            Ok(match self.want {
                Some(want) => found.len() < want,
                None => true,
            })
        });
        if scanned.is_ok() {
            self.body
                .ran
                .fetch_add(ran, std::sync::atomic::Ordering::Relaxed);
        }
        scanned?;
        if !self.order.is_empty() {
            found.sort_by(|a, b| compare_rows(self.order, a, b));
        }
        runs.try_reserve(1)?;
        runs.push((first, found));
        Ok(())
    }

    fn merge(
        &self,
        into: &mut Self::Partial,
        from: Self::Partial,
        _budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        into.runs.try_reserve(from.runs.len())?;
        into.runs.extend(from.runs);
        Ok(())
    }
}

/// A hash join's probe side read from a table's leaves against a table built once.
pub struct TableProbe<'a> {
    pub table: &'a TableSource,
    pub probe: &'a HashProbe<'a>,
}

/// A worker's matches as (probe rowid, build row), each run tagged with its morsel's first leaf.
pub type TableProbePartial = Vec<(usize, Vec<(i64, usize)>)>;

impl Pipeline for TableProbe<'_> {
    type Partial = TableProbePartial;

    fn rows(&self) -> usize {
        self.table.leaves()
    }

    fn empty(&self) -> TableProbePartial {
        Vec::new()
    }

    fn run(
        &self,
        leaves: Range<usize>,
        partial: &mut TableProbePartial,
        budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        let first = leaves.start;
        let mut found = Vec::new();
        self.table.scan(leaves, |rowid, row| {
            let matches = self.probe.matches(row)?;
            if matches.is_empty() {
                return Ok(());
            }
            budget.charge(std::mem::size_of::<(i64, usize)>().saturating_mul(matches.len()))?;
            found.try_reserve(matches.len())?;
            found.extend(matches.iter().map(|build| (rowid, *build)));
            Ok(())
        })?;
        partial.try_reserve(1)?;
        partial.push((first, found));
        Ok(())
    }

    fn merge(
        &self,
        into: &mut TableProbePartial,
        from: TableProbePartial,
        _budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        into.try_reserve(from.len())?;
        into.extend(from);
        Ok(())
    }
}

impl TableProbe<'_> {
    /// The matches as (probe rowid, build row), in probe-rowid order and each probe row's in build
    /// order, as the serial probe gives them.
    pub fn finish(mut partial: TableProbePartial) -> Result<Vec<(i64, usize)>, MorselError> {
        partial.sort_by_key(|(first, _)| *first);
        let total = partial
            .iter()
            .map(|(_, run)| run.len())
            .fold(0usize, usize::saturating_add);
        let mut out = Vec::new();
        out.try_reserve_exact(total)?;
        for (_, run) in partial {
            out.extend(run);
        }
        Ok(out)
    }

    /// The matches as the switch and the measured costs pick the degree.
    pub fn run_mode(
        &self,
        mode: ParallelMode,
        costs: &mut MachineCosts,
        budget: &MemoryBudget,
    ) -> Result<(Vec<(i64, usize)>, RunStats), MorselError> {
        let (partial, stats) = execute(self, mode, costs, budget)?;
        Ok((Self::finish(partial)?, stats))
    }

    /// The matches at a stated degree, its morsels counted in leaf pages.
    pub fn run_at(
        &self,
        degree: Degree,
        budget: &MemoryBudget,
    ) -> Result<(Vec<(i64, usize)>, RunStats), MorselError> {
        let (partial, stats) = execute_at(self, 0, degree, None, budget)?;
        Ok((Self::finish(partial)?, stats))
    }
}

/// A hash join's build side read from a table's leaves by the workers together (QT.6): each
/// worker keeps the rows of its leaves that pass the filter and whose key holds no NULL, charged
/// to the statement's budget, and scatters their entries by partition; the table is then filled
/// partition by partition ([`Partitioner::fill`]). A refusal is typed: the budget passed
/// ([`MorselError::MemoryBudget`]) sends the statement to its serial loop.
pub struct TableBuild<'a> {
    pub table: &'a TableSource,
    /// Terms over the build table's columns, tested before a row is kept.
    pub filter: &'a [Predicate],
    pub key: &'a [usize],
    pub partitioner: &'a Partitioner,
    /// Keep a row whose key holds a NULL, unscattered (lane qs6-12): an outer join's build row
    /// matches nothing then, and still appears.
    pub keep_unkeyed: bool,
}

/// A worker's run of build rows: its first leaf, the rows it kept, their scattered entries.
pub struct BuildRun {
    first: usize,
    rows: Vec<Vec<Value>>,
    scattered: Scattered,
}

impl Pipeline for TableBuild<'_> {
    type Partial = Vec<BuildRun>;

    fn rows(&self) -> usize {
        self.table.leaves()
    }

    fn empty(&self) -> Vec<BuildRun> {
        Vec::new()
    }

    fn run(
        &self,
        leaves: Range<usize>,
        partial: &mut Vec<BuildRun>,
        budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        let first = leaves.start;
        let mut rows: Vec<Vec<Value>> = Vec::new();
        let mut scattered = self.partitioner.empty()?;
        self.table.scan(leaves, |_, row| {
            for term in self.filter {
                if !term.holds(row)? {
                    return Ok(());
                }
            }
            if !self
                .partitioner
                .scatter(row, self.key, rows.len(), &mut scattered, budget)?
                && !self.keep_unkeyed
            {
                return Ok(());
            }
            let bytes = row.iter().fold(
                std::mem::size_of::<Value>().saturating_mul(row.len()),
                |sum, value| {
                    sum.saturating_add(match value {
                        Value::Text(text) => text.as_str().len(),
                        Value::Blob(blob) => blob.len(),
                        _ => 0,
                    })
                },
            );
            budget.charge(bytes)?;
            rows.try_reserve(1)?;
            let mut owned = Vec::new();
            owned.try_reserve_exact(row.len())?;
            owned.extend_from_slice(row);
            rows.push(owned);
            Ok(())
        })?;
        partial.try_reserve(1)?;
        partial.push(BuildRun {
            first,
            rows,
            scattered,
        });
        Ok(())
    }

    fn merge(
        &self,
        into: &mut Vec<BuildRun>,
        from: Vec<BuildRun>,
        _budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        into.try_reserve(from.len())?;
        into.extend(from);
        Ok(())
    }
}

/// What a build did: its scan's run and its fill's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildStats {
    pub scan: RunStats,
    pub fill: RunStats,
}

impl TableBuild<'_> {
    /// The kept build rows in rowid order and their table, scanned and filled as the switch and
    /// the measured costs pick the workers.
    pub fn run_mode(
        &self,
        mode: ParallelMode,
        costs: &mut MachineCosts,
        budget: &MemoryBudget,
    ) -> Result<(Vec<Vec<Value>>, PartitionedTable, BuildStats), MorselError> {
        let (runs, scan) = execute(self, mode, costs, budget)?;
        let (rows, table, fill) = self.finish(runs, mode, costs, budget)?;
        Ok((rows, table, BuildStats { scan, fill }))
    }

    /// The same at a stated degree for the scan (the fill runs serially): a test's fixed machine.
    pub fn run_at(
        &self,
        degree: Degree,
        budget: &MemoryBudget,
    ) -> Result<(Vec<Vec<Value>>, PartitionedTable, BuildStats), MorselError> {
        let (runs, scan) = execute_at(self, 0, degree, None, budget)?;
        let mut costs = MachineCosts::stated(std::num::NonZeroUsize::MIN, 0.0, 0.0);
        let (rows, table, fill) = self.finish(runs, ParallelMode::Off, &mut costs, budget)?;
        Ok((rows, table, BuildStats { scan, fill }))
    }

    /// Puts the runs in leaf order, numbers their rows from there, and fills the table.
    fn finish(
        &self,
        mut runs: Vec<BuildRun>,
        mode: ParallelMode,
        costs: &mut MachineCosts,
        budget: &MemoryBudget,
    ) -> Result<(Vec<Vec<Value>>, PartitionedTable, RunStats), MorselError> {
        runs.sort_by_key(|run| run.first);
        let total = runs
            .iter()
            .map(|run| run.rows.len())
            .fold(0usize, usize::saturating_add);
        let mut rows = Vec::new();
        rows.try_reserve_exact(total)?;
        let mut offsets = Vec::new();
        offsets.try_reserve_exact(runs.len())?;
        let mut scattered = Vec::new();
        scattered.try_reserve_exact(runs.len())?;
        for run in runs {
            offsets.push(rows.len());
            rows.extend(run.rows);
            scattered.push(run.scattered);
        }
        let (table, stats) = self
            .partitioner
            .fill(scattered, &offsets, mode, costs, budget)?;
        Ok((rows, table, stats))
    }
}

/// One link of a join's chain of hash tables (QT.6, lane qs6-10): the table, keyed by places in
/// the row joined before it (the probe row's columns, then each earlier link's build row's), and
/// the build rows it numbers.
pub struct JoinStage<'a> {
    pub probe: HashProbe<'a>,
    pub rows: Vec<Vec<Value>>,
    /// A link read by a rowid seek per joined row instead of its hash table (lane qs6-16,
    /// `SearchedLinks::Seek`); `probe` and `rows` are then empty. Inner joins only.
    pub seek: Option<SeekLink>,
}

/// A link read by a seek per joined row (lane qs6-16): its table's row whose rowid is the joined
/// row's value at `rowid_at`, a match when its other key pairs are equal as the hash table's keys
/// are (`key_part`: no NULL matches, an integral REAL is its INTEGER) and its own terms hold. A
/// rowid is unique, so a joined row has one match at most.
pub struct SeekLink {
    pub table: TableSource,
    pub rowid_at: usize,
    /// The other key pairs: a place in the joined row and the link's column.
    pub also: Vec<(usize, usize)>,
    pub filter: Vec<Predicate>,
}

/// A worker's cursor and row for each seek link of a chain, by the link's place (lane qs6-16).
pub struct SeekSlot {
    cursor: Option<BTreeCursor>,
    row: Vec<Value>,
}

/// One empty slot per link of `stages`.
fn seek_slots(stages: &[JoinStage<'_>]) -> Result<Vec<SeekSlot>, MorselError> {
    let mut slots = Vec::new();
    slots.try_reserve_exact(stages.len())?;
    slots.extend(stages.iter().map(|_| SeekSlot {
        cursor: None,
        row: Vec::new(),
    }));
    Ok(slots)
}

const ONE_MATCH: &[usize] = &[0];
const NO_MATCH: &[usize] = &[];

impl SeekLink {
    /// Whether the joined row `joined` has its match, read into `slot.row`.
    fn find(&self, slot: &mut SeekSlot, joined: &[Value]) -> Result<bool, MorselError> {
        let sought = joined.get(self.rowid_at).ok_or(MorselError::NoColumn {
            column: self.rowid_at,
            width: joined.len(),
        })?;
        let KeyPart::Int(rowid) = key_part(sought) else {
            return Ok(false);
        };
        let cursor = slot.cursor.get_or_insert_with(|| self.table.cursor());
        if !self.table.seek_row(cursor, rowid, &mut slot.row)? {
            return Ok(false);
        }
        for (at, column) in &self.also {
            let (Some(a), Some(b)) = (joined.get(*at), slot.row.get(*column)) else {
                return Err(MorselError::NoColumn {
                    column: *column,
                    width: slot.row.len(),
                });
            };
            let a = key_part(a);
            if matches!(a, KeyPart::Null) || a != key_part(b) {
                return Ok(false);
            }
        }
        for term in &self.filter {
            if !term.holds(&slot.row)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// The matches of link `level` (`stage`) for the joined row so far: its hash table's, or its
/// seek's one (lane qs6-16).
fn stage_matches<'s>(
    stage: &'s JoinStage<'_>,
    level: usize,
    joined: &[Value],
    seeks: &mut [SeekSlot],
) -> Result<&'s [usize], MorselError> {
    let Some(link) = &stage.seek else {
        return stage.probe.matches(joined);
    };
    let slot = seeks.get_mut(level).ok_or(MorselError::Overflow {
        what: "a join's seek link",
    })?;
    Ok(if link.find(slot, joined)? {
        ONE_MATCH
    } else {
        NO_MATCH
    })
}

/// A link's place in a probe row's walk through the chain: its matches, the next one to take, and
/// the joined row's length before the link's build row.
type Step<'s> = (&'s [usize], usize, usize);

/// Calls `visit` with each joined row the probe row `row` makes through the chain `stages`, depth
/// first: the first link's matches in build order, each one's matches in the next link, and so on.
/// The walk is an explicit stack as deep as the chain, no recursion; `joined` and `walk` are the
/// worker's, reused from row to row.
fn each_joined<'s>(
    stages: &'s [JoinStage<'_>],
    row: &[Value],
    joined: &mut Vec<Value>,
    (walk, seeks): (&mut Vec<Step<'s>>, &mut [SeekSlot]),
    mut visit: impl FnMut(&[Value]) -> Result<(), MorselError>,
) -> Result<(), MorselError> {
    joined.clear();
    joined.try_reserve(row.len())?;
    joined.extend_from_slice(row);
    walk.clear();
    let Some(first) = stages.first() else {
        return visit(joined);
    };
    walk.try_reserve(stages.len())?;
    walk.push((stage_matches(first, 0, joined, seeks)?, 0, joined.len()));
    while let Some(top) = walk.last_mut() {
        let (matches, next, base) = *top;
        let Some(build) = matches.get(next) else {
            walk.pop();
            continue;
        };
        top.1 = next.saturating_add(1);
        let level = walk.len().saturating_sub(1);
        let stage = stages.get(level).ok_or(MorselError::Overflow {
            what: "a join's link",
        })?;
        // A seek link's one match is the row its slot holds (lane qs6-16).
        let build_row = match &stage.seek {
            None => stage.rows.get(*build).map(Vec::as_slice),
            Some(_) => seeks.get(level).map(|slot| slot.row.as_slice()),
        }
        .ok_or(MorselError::Overflow {
            what: "a match's build row",
        })?;
        joined.truncate(base);
        joined.try_reserve(build_row.len())?;
        joined.extend_from_slice(build_row);
        let deeper = level.saturating_add(1);
        match stages.get(deeper) {
            None => visit(joined)?,
            Some(link) => {
                let found = stage_matches(link, deeper, joined, seeks)?;
                walk.push((found, 0, joined.len()));
            }
        }
    }
    Ok(())
}

/// Calls `visit` with each joined row the probe row `row` makes, as [`each_joined`] does. With
/// `pad_last` (lane qs6-13, a LEFT join among more than two tables: the last link's columns), each
/// joined row of the links before the last is probed in the last link; `visit` says whether a
/// joined row matched (passed the ON terms), and a row of the others that no match passed, or that
/// has none, is visited once more padded with NULLs for the last link's columns (`padded` true).
/// The scratch rows and the walk are the worker's, reused from row to row.
#[allow(clippy::type_complexity)]
fn each_row<'s>(
    stages: &'s [JoinStage<'_>],
    pad_last: Option<usize>,
    row: &[Value],
    (joined, last_row, walk, seeks): (
        &mut Vec<Value>,
        &mut Vec<Value>,
        &mut Vec<Step<'s>>,
        &mut [SeekSlot],
    ),
    mut visit: impl FnMut(&[Value], bool) -> Result<bool, MorselError>,
) -> Result<(), MorselError> {
    let (Some(width), Some((last, before))) = (pad_last, stages.split_last()) else {
        return each_joined(stages, row, joined, (walk, seeks), |joined| {
            visit(joined, false).map(|_| ())
        });
    };
    // The padded last link is a hash link: a seek link is an inner join's only (lane qs6-16).
    if last.seek.is_some() {
        return Err(MorselError::Corrupt("a padded link read by a seek"));
    }
    each_joined(before, row, joined, (walk, seeks), |prefix| {
        let mut matched = false;
        for build in last.probe.matches(prefix)? {
            let build_row = last.rows.get(*build).ok_or(MorselError::Overflow {
                what: "a match's build row",
            })?;
            last_row.clear();
            last_row.try_reserve(prefix.len().saturating_add(build_row.len()))?;
            last_row.extend_from_slice(prefix);
            last_row.extend_from_slice(build_row);
            matched |= visit(last_row, false)?;
        }
        if !matched {
            let padded = prefix.len().saturating_add(width);
            last_row.clear();
            last_row.try_reserve(padded)?;
            last_row.extend_from_slice(prefix);
            last_row.resize(padded, Value::Null);
            visit(last_row, true)?;
        }
        Ok(())
    })
}

/// A join's probe side read from a table's leaves, each joined row (the probe row's columns, then
/// each link's build row's) run through the copy of the serial loop's body (lane qs6-12): its
/// filter terms the hash keys and the tables' own `Predicate`s don't hold, its GROUP BY values and
/// aggregate inputs, any expression the serial loop takes. The rows the body keeps fold into the
/// worker's partial aggregates. The probe through the chain and the body are one loop, run by
/// every worker over its own leaves (QT.6).
pub struct TableJoinAggregate<'a> {
    pub table: &'a TableSource,
    /// Terms over the probe row alone, tested before it is probed.
    pub probe_filter: &'a [Predicate],
    /// The chain of hash tables, one link per build side.
    pub stages: &'a [JoinStage<'a>],
    /// The loop body over a joined row, its columns written from `columns_start` on, and the
    /// grouping and aggregates over its outputs.
    pub body: &'a TableBodyAggregate<'a>,
    /// A LEFT join among more than two tables (lane qs6-13, `MorselOuter::LeftLink`): the last
    /// link's columns, the padded table's.
    pub pad_last: Option<usize>,
}

impl Pipeline for TableJoinAggregate<'_> {
    type Partial = BodyPartial;

    fn rows(&self) -> usize {
        self.table.leaves()
    }

    fn empty(&self) -> Self::Partial {
        BodyPartial::default()
    }

    fn run(
        &self,
        leaves: Range<usize>,
        partial: &mut Self::Partial,
        budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        let BodyPartial { aggregate, worker } = partial;
        let worker = match worker {
            Some(worker) => worker,
            None => worker.insert(Box::new(self.body.worker()?)),
        };
        let mut joined: Vec<Value> = Vec::new();
        let mut last: Vec<Value> = Vec::new();
        let mut walk = Vec::new();
        let mut seeks = seek_slots(self.stages)?;
        let mut ran = 0u64;
        let scanned = self.table.scan(leaves, |rowid, row| {
            for term in self.probe_filter {
                if !term.holds(row)? {
                    return Ok(());
                }
            }
            // A probe row's joined rows share its place: a value the body's outputs could tie
            // on and still differ in (a REAL beside an INTEGER) sends the statement to the serial
            // loop (`MorselBody::no_real`), so a tie among them is no choice.
            each_row(
                self.stages,
                self.pad_last,
                row,
                (&mut joined, &mut last, &mut walk, seeks.as_mut_slice()),
                |joined, padded| {
                    ran = ran.saturating_add(1);
                    let (matched, passed) =
                        self.body
                            .joined_row(worker, rowid, joined, self.pad_last, padded)?;
                    if passed {
                        self.body
                            .shape
                            .fold(&worker.out, position(rowid)?, aggregate, budget)?;
                    }
                    Ok(matched)
                },
            )
        });
        if scanned.is_ok() {
            self.body
                .ran
                .fetch_add(ran, std::sync::atomic::Ordering::Relaxed);
        }
        scanned
    }

    fn merge(
        &self,
        into: &mut Self::Partial,
        from: Self::Partial,
        budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        self.body
            .shape
            .merge(&mut into.aggregate, from.aggregate, budget)
    }
}

impl TableJoinAggregate<'_> {
    /// The answer as the switch and the measured costs pick the degree.
    pub fn run_mode(
        &self,
        mode: ParallelMode,
        costs: &mut MachineCosts,
        budget: &MemoryBudget,
    ) -> Result<(Vec<Vec<Value>>, RunStats), MorselError> {
        let (partial, stats) = execute(self, mode, costs, budget)?;
        Ok((self.body.shape.finish(partial.aggregate)?, stats))
    }
}

/// A hash join's rows, no aggregates (QT.6): each worker probes its own leaves, runs each joined
/// row through the copy of the serial loop's body (lane qs6-12) and keeps the outputs of the rows
/// it passes (the result columns, then the ORDER BY keys); with ORDER BY each worker sorts its
/// morsel's rows, and the runs are merged in leaf order by a stable merge, so rows equal under the
/// ORDER BY keys keep the serial loop's probe order. Without ORDER BY the runs are concatenated in
/// leaf order: probe-row order, each probe row's matches in build order.
pub struct TableJoinRows<'a> {
    pub table: &'a TableSource,
    /// Terms over the probe row alone, tested before it is probed.
    pub probe_filter: &'a [Predicate],
    /// The chain of hash tables, one link per build side.
    pub stages: &'a [JoinStage<'a>],
    /// The loop body over a joined row; its outputs are the output row.
    pub body: &'a TableBodyAggregate<'a>,
    /// As [`TableJoinAggregate::pad_last`].
    pub pad_last: Option<usize>,
    /// The sort keys: each one's place in the output row, and whether it descends.
    pub order: &'a [(usize, bool)],
}

/// A worker's output rows, each run tagged with its morsel's first leaf, and the worker that runs
/// the body.
#[derive(Default)]
pub struct JoinRowsPartial {
    runs: Vec<(usize, Vec<Vec<Value>>)>,
    worker: Option<Box<BodyWorker>>,
}

impl TableJoinRows<'_> {
    fn compare(&self, a: &[Value], b: &[Value]) -> std::cmp::Ordering {
        compare_rows(self.order, a, b)
    }

    /// The rows in leaf order, sorted when the query asks: `sort_by` is a stable merge sort that
    /// finds the workers' sorted runs and merges them.
    pub fn finish(&self, mut partial: JoinRowsPartial) -> Result<Vec<Vec<Value>>, MorselError> {
        partial.runs.sort_by_key(|(first, _)| *first);
        let total = partial
            .runs
            .iter()
            .map(|(_, run)| run.len())
            .fold(0usize, usize::saturating_add);
        let mut out = Vec::new();
        out.try_reserve_exact(total)?;
        for (_, run) in partial.runs {
            out.extend(run);
        }
        if !self.order.is_empty() {
            out.sort_by(|a, b| self.compare(a, b));
        }
        Ok(out)
    }

    /// The rows as the switch and the measured costs pick the degree.
    pub fn run_mode(
        &self,
        mode: ParallelMode,
        costs: &mut MachineCosts,
        budget: &MemoryBudget,
    ) -> Result<(Vec<Vec<Value>>, RunStats), MorselError> {
        let (partial, stats) = execute(self, mode, costs, budget)?;
        Ok((self.finish(partial)?, stats))
    }
}

impl Pipeline for TableJoinRows<'_> {
    type Partial = JoinRowsPartial;

    fn rows(&self) -> usize {
        self.table.leaves()
    }

    fn empty(&self) -> JoinRowsPartial {
        JoinRowsPartial::default()
    }

    fn run(
        &self,
        leaves: Range<usize>,
        partial: &mut JoinRowsPartial,
        budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        let JoinRowsPartial { runs, worker } = partial;
        let worker = match worker {
            Some(worker) => worker,
            None => worker.insert(Box::new(self.body.worker()?)),
        };
        let first = leaves.start;
        let mut found: Vec<Vec<Value>> = Vec::new();
        let mut joined: Vec<Value> = Vec::new();
        let mut last: Vec<Value> = Vec::new();
        let mut walk = Vec::new();
        let mut seeks = seek_slots(self.stages)?;
        let mut ran = 0u64;
        let scanned = self.table.scan(leaves, |rowid, row| {
            for term in self.probe_filter {
                if !term.holds(row)? {
                    return Ok(());
                }
            }
            each_row(
                self.stages,
                self.pad_last,
                row,
                (&mut joined, &mut last, &mut walk, seeks.as_mut_slice()),
                |joined, padded| {
                    ran = ran.saturating_add(1);
                    let (matched, passed) =
                        self.body
                            .joined_row(worker, rowid, joined, self.pad_last, padded)?;
                    if passed {
                        found.try_reserve(1)?;
                        found.push(kept(&worker.out, budget)?);
                    }
                    Ok(matched)
                },
            )
        });
        if scanned.is_ok() {
            self.body
                .ran
                .fetch_add(ran, std::sync::atomic::Ordering::Relaxed);
        }
        scanned?;
        if !self.order.is_empty() {
            found.sort_by(|a, b| self.compare(a, b));
        }
        runs.try_reserve(1)?;
        runs.push((first, found));
        Ok(())
    }

    fn merge(
        &self,
        into: &mut JoinRowsPartial,
        from: JoinRowsPartial,
        _budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        into.runs.try_reserve(from.runs.len())?;
        into.runs.extend(from.runs);
        Ok(())
    }
}

/// Amphora (QT.6, lane qs6-15): what the outer joins on morsels did, by orientation
/// ([`crate::OuterJoinProbe`]), for the measurements that decide between them: the rows each way
/// of padding padded, the bitmaps' size, and the statements each path ran. Counted per morsel,
/// added once per morsel; a statement's share is the difference across it.
#[derive(Debug, Default)]
pub struct OuterCounters {
    /// Rows padded during the walk: a row of the tables before a link that keeps them that no
    /// match passed (the chain's LEFT and FULL links), or a FULL join's probe row that matched
    /// nothing (two tables).
    pub walk_pads: std::sync::atomic::AtomicU64,
    /// Build rows padded once the bitmaps merged: rows of a table the join keeps that no row
    /// matched.
    pub bitmap_pads: std::sync::atomic::AtomicU64,
    /// The bitmap words the workers sized, a word per 64 build rows per worker per phase: what
    /// marking in a bitmap holds.
    pub bitmap_words: std::sync::atomic::AtomicU64,
    /// Statements whose outer join of two tables probed the table it pads ([`TableOuterJoin`]).
    pub padded_probes: std::sync::atomic::AtomicU64,
    /// Statements run as a chain in the FROM clause's order ([`TableChain`]), a two-table outer
    /// join that probes its kept table among them.
    pub chains: std::sync::atomic::AtomicU64,
    /// The bytes the outer-join statements charged to their memory budgets, summed (lane qs6-16):
    /// the hash tables, the bitmaps and the partials. A budget is charged and never credited back
    /// within its statement, so a statement's charge is all it held at once at most.
    pub charged_bytes: std::sync::atomic::AtomicU64,
    /// The most bytes any one outer-join statement charged (lane qs6-16); a reader may store 0 to
    /// start a fresh maximum.
    pub charged_peak: std::sync::atomic::AtomicU64,
}

impl OuterCounters {
    fn add(counter: &std::sync::atomic::AtomicU64, by: u64) {
        if by > 0 {
            counter.fetch_add(by, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// An outer join of two tables on morsels (lane qs6-12): the probe is the table the join pads with
/// NULLs, its one link the table whose every row appears. Each worker probes its own leaves; a
/// joined row that passes the ON terms (the body from `MorselBody::on`) matched, and its build row
/// is marked in the worker's own bitmap; it then runs the rest of the body (the WHERE terms, the
/// outputs). A FULL join's probe row that matched nothing comes out padded with NULLs for the
/// build's side. Once every probe morsel ran, the bitmaps are merged (`merge`, an OR), and the
/// build rows no worker marked run the body padded with NULLs for the probe's side
/// ([`TableOuterRest`], in morsels of build rows).
pub struct TableOuterJoin<'a> {
    pub table: &'a TableSource,
    /// Terms over the probe row alone, tested before it is probed (a LEFT join's ON terms on it).
    pub probe_filter: &'a [Predicate],
    /// The link: the build table's hash table and rows, every row kept, a NULL key's too.
    pub stage: &'a JoinStage<'a>,
    /// The probe's and the build's widths, the NULLs a padded row takes for each.
    pub probe_columns: usize,
    pub build_columns: usize,
    pub kind: MorselOuter,
    /// The loop body over a joined row, and the grouping and aggregates over its outputs.
    pub body: &'a TableBodyAggregate<'a>,
    /// A join's rows: the sort keys, each one's place in the output row and whether it descends.
    pub order: &'a [(usize, bool)],
    /// What the orientations did (lane qs6-15).
    pub counts: &'a OuterCounters,
}

/// The partial of [`TableOuterJoin`] and [`TableOuterRest`]: the aggregates, or the output rows
/// tagged with their morsel's first leaf or build row; the build rows matched, a bit per row; and
/// the worker that runs the body.
#[derive(Default)]
pub struct OuterPartial {
    aggregate: super::morsel::AggregatePartial,
    runs: Vec<(usize, Vec<Vec<Value>>)>,
    matched: Vec<u64>,
    worker: Option<Box<BodyWorker>>,
}

/// A build row's word and bit in a matched-row bitmap.
fn matched_bit(row: usize) -> Result<(usize, u64), MorselError> {
    let word = row.checked_div(64).ok_or(MorselError::Overflow {
        what: "a matched row's word",
    })?;
    let shift = row
        .checked_rem(64)
        .and_then(|bit| u32::try_from(bit).ok())
        .and_then(|bit| 1u64.checked_shl(bit))
        .ok_or(MorselError::Overflow {
            what: "a matched row's bit",
        })?;
    Ok((word, shift))
}

impl TableOuterJoin<'_> {
    /// Keeps the body's outputs: folded into the aggregates, or as an output row.
    fn keep(
        &self,
        out: &[Value],
        position: usize,
        aggregate: &mut super::morsel::AggregatePartial,
        found: &mut Vec<Vec<Value>>,
        budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        if self.body.body.rows {
            found.try_reserve(1)?;
            found.push(kept(out, budget)?);
            Ok(())
        } else {
            self.body.shape.fold(out, position, aggregate, budget)
        }
    }

    /// Merges what the padded build rows gave into what the probe gave.
    fn merge_into(
        &self,
        into: &mut OuterPartial,
        from: OuterPartial,
        budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        self.body
            .shape
            .merge(&mut into.aggregate, from.aggregate, budget)?;
        into.runs.try_reserve(from.runs.len())?;
        into.runs.extend(from.runs);
        Ok(())
    }

    /// The answer: the probe's morsels, then the build rows no probe row matched, as the switch
    /// and the measured costs pick the degree of each.
    pub fn run_mode(
        &self,
        mode: ParallelMode,
        costs: &mut MachineCosts,
        budget: &MemoryBudget,
    ) -> Result<(Vec<Vec<Value>>, RunStats), MorselError> {
        let (mut probed, stats) = execute(self, mode, costs, budget)?;
        let matched = std::mem::take(&mut probed.matched);
        let rest = TableOuterRest {
            join: self,
            matched: &matched,
        };
        let (padded, _) = execute(&rest, mode, costs, budget)?;
        if !self.body.body.rows {
            self.body
                .shape
                .merge(&mut probed.aggregate, padded.aggregate, budget)?;
            return Ok((self.body.shape.finish(probed.aggregate)?, stats));
        }
        // The probe's rows in leaf order, then the padded build rows in build order.
        probed.runs.sort_by_key(|(first, _)| *first);
        let mut tail = padded.runs;
        tail.sort_by_key(|(first, _)| *first);
        let total = probed
            .runs
            .iter()
            .chain(tail.iter())
            .map(|(_, run)| run.len())
            .fold(0usize, usize::saturating_add);
        let mut out = Vec::new();
        out.try_reserve_exact(total)?;
        for (_, run) in probed.runs.into_iter().chain(tail) {
            out.extend(run);
        }
        if !self.order.is_empty() {
            out.sort_by(|a, b| compare_rows(self.order, a, b));
        }
        Ok((out, stats))
    }
}

impl Pipeline for TableOuterJoin<'_> {
    type Partial = OuterPartial;

    fn rows(&self) -> usize {
        self.table.leaves()
    }

    fn empty(&self) -> Self::Partial {
        OuterPartial::default()
    }

    fn run(
        &self,
        leaves: Range<usize>,
        partial: &mut Self::Partial,
        budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        let OuterPartial {
            aggregate,
            runs,
            matched,
            worker,
        } = partial;
        let worker = match worker {
            Some(worker) => worker,
            None => worker.insert(Box::new(self.body.worker()?)),
        };
        // The worker's bitmap, a bit per build row, charged to the statement's budget.
        let words = self.stage.rows.len().div_ceil(64);
        if matched.len() < words {
            budget.charge(words.saturating_mul(std::mem::size_of::<u64>()))?;
            matched.try_reserve_exact(words)?;
            matched.resize(words, 0);
            OuterCounters::add(
                &self.counts.bitmap_words,
                u64::try_from(words).unwrap_or(u64::MAX),
            );
        }
        let full = self.kind == MorselOuter::Full;
        let first = leaves.start;
        let mut found: Vec<Vec<Value>> = Vec::new();
        let mut joined: Vec<Value> = Vec::new();
        let mut ran = 0u64;
        let mut pads = 0u64;
        let scanned = self.table.scan(leaves, |rowid, row| {
            for term in self.probe_filter {
                if !term.holds(row)? {
                    return Ok(());
                }
            }
            let mut any = false;
            for build in self.stage.probe.matches(row)? {
                let build_row = self.stage.rows.get(*build).ok_or(MorselError::Overflow {
                    what: "a match's build row",
                })?;
                joined.clear();
                joined.try_reserve(row.len().saturating_add(build_row.len()))?;
                joined.extend_from_slice(row);
                joined.extend_from_slice(build_row);
                ran = ran.saturating_add(1);
                let (hit, passed) = self.body.on_row(worker, Some(rowid), &joined)?;
                if hit {
                    any = true;
                    let (word, bit) = matched_bit(*build)?;
                    let at = matched.get_mut(word).ok_or(MorselError::Overflow {
                        what: "a matched row's word",
                    })?;
                    *at |= bit;
                }
                if passed {
                    self.keep(&worker.out, position(rowid)?, aggregate, &mut found, budget)?;
                }
            }
            // A FULL join's probe row that matched nothing, padded with the build's NULLs.
            if full && !any {
                joined.clear();
                joined.try_reserve(row.len().saturating_add(self.build_columns))?;
                joined.extend_from_slice(row);
                joined.resize(row.len().saturating_add(self.build_columns), Value::Null);
                ran = ran.saturating_add(1);
                pads = pads.saturating_add(1);
                if self.body.padded_row(worker, Some(rowid), &joined)? {
                    self.keep(&worker.out, position(rowid)?, aggregate, &mut found, budget)?;
                }
            }
            Ok(())
        });
        if scanned.is_ok() {
            self.body
                .ran
                .fetch_add(ran, std::sync::atomic::Ordering::Relaxed);
            OuterCounters::add(&self.counts.walk_pads, pads);
        }
        scanned?;
        if !self.order.is_empty() {
            found.sort_by(|a, b| compare_rows(self.order, a, b));
        }
        runs.try_reserve(1)?;
        runs.push((first, found));
        Ok(())
    }

    fn merge(
        &self,
        into: &mut Self::Partial,
        mut from: Self::Partial,
        budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        // The matched-row bitmaps merge by OR: a build row matched when any worker matched it.
        or_marks(&mut into.matched, std::mem::take(&mut from.matched));
        self.merge_into(into, from, budget)
    }
}

/// The build rows of an outer join that no probe row matched (lane qs6-12), in morsels of build
/// rows: each runs the body padded with NULLs for the probe's side, its rowid NULL.
pub struct TableOuterRest<'a> {
    pub join: &'a TableOuterJoin<'a>,
    /// The merged bitmap of the build rows matched.
    pub matched: &'a [u64],
}

impl Pipeline for TableOuterRest<'_> {
    type Partial = OuterPartial;

    fn rows(&self) -> usize {
        self.join.stage.rows.len()
    }

    fn empty(&self) -> Self::Partial {
        OuterPartial::default()
    }

    fn run(
        &self,
        rows: Range<usize>,
        partial: &mut Self::Partial,
        budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        let OuterPartial {
            aggregate,
            runs,
            worker,
            ..
        } = partial;
        let body = self.join.body;
        let worker = match worker {
            Some(worker) => worker,
            None => worker.insert(Box::new(body.worker()?)),
        };
        let first = rows.start;
        let mut found: Vec<Vec<Value>> = Vec::new();
        let mut joined: Vec<Value> = Vec::new();
        let mut ran = 0u64;
        for at in rows {
            let (word, bit) = matched_bit(at)?;
            if self.matched.get(word).is_some_and(|w| w & bit != 0) {
                continue;
            }
            let build_row = self.join.stage.rows.get(at).ok_or(MorselError::Overflow {
                what: "an unmatched build row",
            })?;
            joined.clear();
            joined.try_reserve(self.join.probe_columns.saturating_add(build_row.len()))?;
            joined.resize(self.join.probe_columns, Value::Null);
            joined.extend_from_slice(build_row);
            ran = ran.saturating_add(1);
            if body.padded_row(worker, None, &joined)? {
                self.join
                    .keep(&worker.out, at, aggregate, &mut found, budget)?;
            }
        }
        body.ran
            .fetch_add(ran, std::sync::atomic::Ordering::Relaxed);
        // Every row this morsel ran was padded after the bitmaps merged.
        OuterCounters::add(&self.join.counts.bitmap_pads, ran);
        if !self.join.order.is_empty() {
            found.sort_by(|a, b| compare_rows(self.join.order, a, b));
        }
        runs.try_reserve(1)?;
        runs.push((first, found));
        Ok(())
    }

    fn merge(
        &self,
        into: &mut Self::Partial,
        from: Self::Partial,
        budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        self.join.merge_into(into, from, budget)
    }
}

/// Joins of three or more tables with a RIGHT or FULL JOIN among them, or with more than one outer
/// join (lane qs6-14, T-QT.6.24 and T-QT.6.25; [`MorselOuter::Chain`]): the probe is the FROM
/// clause's first table and each link the next one in turn. A probe row walks the chain depth
/// first; at each link a match that passes the link's own ON terms goes on to the next link, and
/// a row of the tables before it that no match passed goes on padded with NULLs when the link
/// keeps the left rows. A link that keeps the right rows marks each build row matched in the
/// worker's own bitmap. The walk runs in phases: the probe's leaves first, then for each link
/// that keeps the right rows, in the chain's order, its build rows that no worker marked (the
/// bitmaps merged once every earlier phase ran, so every row that can reach the link has), padded
/// with NULLs for the tables before it, walk the rest of the chain. Every row that reaches the
/// chain's end runs the WHERE terms and the outputs.
pub struct TableChain<'a> {
    pub table: &'a TableSource,
    /// Terms over the probe row alone, tested before it walks (WHERE terms, only when no link
    /// keeps the right rows: those links' padded rows don't pass the probe's test).
    pub probe_filter: &'a [Predicate],
    /// The chain of hash tables, one link per table after the probe.
    pub stages: &'a [JoinStage<'a>],
    /// Each link's keeps and its table's columns, the NULLs a padded row takes for it.
    pub links: &'a [(super::insn::MorselKeeps, usize)],
    /// The probe table's columns.
    pub probe_columns: usize,
    /// The loop body: each link's ON code (`MorselBody::links_on`), then the WHERE terms and
    /// the outputs over the whole joined row.
    pub body: &'a TableBodyAggregate<'a>,
    /// A join's rows: the sort keys, each one's place in the output row and whether it descends.
    pub order: &'a [(usize, bool)],
    /// What the orientations did (lane qs6-15).
    pub counts: &'a OuterCounters,
}

/// A walk's place at one link: the link's matches for the row before it, the next one to take,
/// the joined row's length before the link's columns, and whether any match passed.
type ChainFrame<'s> = (&'s [usize], usize, usize, bool);

/// The partial of a [`TableChain`]'s phase: the aggregates, or the output rows tagged with their
/// morsel's first leaf or build row; per link, the build rows its rows matched, a bit per row
/// (empty for a link that keeps no right rows); and the worker that runs the body.
#[derive(Default)]
pub struct ChainPartial {
    aggregate: super::morsel::AggregatePartial,
    runs: Vec<(usize, Vec<Vec<Value>>)>,
    matched: Vec<Vec<u64>>,
    worker: Option<Box<BodyWorker>>,
}

/// A worker's walk through a [`TableChain`]: its body worker, what it keeps, its marks, the
/// joined row and the walk's frames (reused from row to row), and the joined rows it ran.
struct ChainWalk<'s, 'w> {
    worker: &'w mut BodyWorker,
    aggregate: &'w mut super::morsel::AggregatePartial,
    matched: &'w mut [Vec<u64>],
    found: Vec<Vec<Value>>,
    joined: Vec<Value>,
    frames: Vec<ChainFrame<'s>>,
    ran: u64,
    /// The rows padded during the walk (lane qs6-15).
    pads: u64,
}

impl<'a> TableChain<'a> {
    /// The joined row's width before link `link`: the probe's columns and each earlier link's.
    fn before(&self, link: usize) -> Result<usize, MorselError> {
        self.links
            .iter()
            .take(link)
            .try_fold(self.probe_columns, |sum, (_, columns)| {
                sum.checked_add(*columns)
            })
            .ok_or(MorselError::Overflow {
                what: "a joined row's width",
            })
    }

    /// Whether the joined row passes link `link`'s own ON code (none: it does).
    fn link_on(
        &self,
        worker: &mut BodyWorker,
        link: usize,
        rowid: Option<i64>,
        row: &[Value],
    ) -> Result<bool, MorselError> {
        let Some(Some((from, pass))) = self.body.body.links_on.get(link) else {
            return Ok(true);
        };
        self.body.load(worker, rowid, row)?;
        self.body.run_to(worker, *from, *pass)
    }

    /// The joined row in `walk.joined` (the tables before link `link`) at link `link`: its
    /// matches there pushed for the walk, or, past the last link, the WHERE terms and the outputs
    /// run over it and the outputs kept.
    fn enter(
        &self,
        walk: &mut ChainWalk<'a, '_>,
        link: usize,
        rowid: Option<i64>,
        position: usize,
        budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        let Some(stage) = self.stages.get(link) else {
            walk.ran = walk.ran.saturating_add(1);
            self.body.load(walk.worker, rowid, &walk.joined)?;
            if !self
                .body
                .passes(walk.worker, self.body.body.start, self.body.body.keep)?
            {
                return Ok(());
            }
            return if self.body.body.rows {
                walk.found.try_reserve(1)?;
                walk.found.push(kept(&walk.worker.out, budget)?);
                Ok(())
            } else {
                self.body
                    .shape
                    .fold(&walk.worker.out, position, walk.aggregate, budget)
            };
        };
        let matches = stage.probe.matches(&walk.joined)?;
        walk.frames.try_reserve(1)?;
        walk.frames.push((matches, 0, walk.joined.len(), false));
        Ok(())
    }

    /// Walks the joined row in `walk.joined` (the tables before link `from`) through the links
    /// from `from` on, depth first, an explicit stack as deep as the chain (no recursion).
    fn walk(
        &self,
        walk: &mut ChainWalk<'a, '_>,
        from: usize,
        rowid: Option<i64>,
        position: usize,
        budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        walk.frames.clear();
        self.enter(walk, from, rowid, position, budget)?;
        while let Some(&(matches, next, base, any)) = walk.frames.last() {
            let link = from.saturating_add(walk.frames.len().saturating_sub(1));
            let (Some(stage), Some((keeps, columns))) =
                (self.stages.get(link), self.links.get(link))
            else {
                return Err(MorselError::Overflow {
                    what: "a join's link",
                });
            };
            if let Some(build) = matches.get(next) {
                if let Some(top) = walk.frames.last_mut() {
                    top.1 = next.saturating_add(1);
                }
                let build_row = stage.rows.get(*build).ok_or(MorselError::Overflow {
                    what: "a match's build row",
                })?;
                walk.joined.truncate(base);
                walk.joined.try_reserve(build_row.len())?;
                walk.joined.extend_from_slice(build_row);
                if !self.link_on(walk.worker, link, rowid, &walk.joined)? {
                    continue;
                }
                if let Some(top) = walk.frames.last_mut() {
                    top.3 = true;
                }
                if keeps.right {
                    let (word, bit) = matched_bit(*build)?;
                    let at = walk
                        .matched
                        .get_mut(link)
                        .and_then(|marks| marks.get_mut(word))
                        .ok_or(MorselError::Overflow {
                            what: "a matched row's word",
                        })?;
                    *at |= bit;
                }
                self.enter(walk, link.saturating_add(1), rowid, position, budget)?;
            } else if !any && keeps.left {
                // No match passed: the row goes on once, padded with NULLs for this link.
                if let Some(top) = walk.frames.last_mut() {
                    top.3 = true;
                }
                walk.pads = walk.pads.saturating_add(1);
                walk.joined.truncate(base);
                walk.joined
                    .resize(base.saturating_add(*columns), Value::Null);
                self.enter(walk, link.saturating_add(1), rowid, position, budget)?;
            } else {
                walk.frames.pop();
            }
        }
        Ok(())
    }

    /// The answer: the probe's phase, then each link's that keeps the right rows, in the chain's
    /// order, the bitmaps merged between phases, as the switch and the measured costs pick each
    /// phase's degree. A join's rows: each phase's in its morsels' order, phase after phase, then
    /// sorted (stably) when the query asks.
    pub fn run_mode(
        &self,
        mode: ParallelMode,
        costs: &mut MachineCosts,
        budget: &MemoryBudget,
    ) -> Result<(Vec<Vec<Value>>, RunStats), MorselError> {
        let mut matched: Vec<Vec<u64>> = Vec::new();
        let mut aggregate = super::morsel::AggregatePartial::default();
        let mut runs: Vec<(usize, usize, Vec<Vec<Value>>)> = Vec::new();
        let mut stats = None;
        let tails = self
            .links
            .iter()
            .enumerate()
            .filter(|(_, (keeps, _))| keeps.right)
            .map(|(link, _)| Some(link));
        for (phase, from) in std::iter::once(None).chain(tails).enumerate() {
            let (partial, ran) = {
                let pipeline = ChainPhase {
                    chain: self,
                    from,
                    matched: &matched,
                };
                execute(&pipeline, mode, costs, budget)?
            };
            // The probe's phase's run stands for the statement's.
            if stats.is_none() {
                stats = Some(ran);
            }
            let ChainPartial {
                aggregate: more,
                runs: rows,
                matched: marks,
                ..
            } = partial;
            or_link_marks(&mut matched, marks);
            self.body.shape.merge(&mut aggregate, more, budget)?;
            runs.try_reserve(rows.len())?;
            runs.extend(rows.into_iter().map(|(first, run)| (phase, first, run)));
        }
        let stats = stats.unwrap_or_default();
        if !self.body.body.rows {
            return Ok((self.body.shape.finish(aggregate)?, stats));
        }
        runs.sort_by_key(|(phase, first, _)| (*phase, *first));
        let total = runs
            .iter()
            .map(|(_, _, run)| run.len())
            .fold(0usize, usize::saturating_add);
        let mut out = Vec::new();
        out.try_reserve_exact(total)?;
        for (_, _, run) in runs {
            out.extend(run);
        }
        if !self.order.is_empty() {
            out.sort_by(|a, b| compare_rows(self.order, a, b));
        }
        Ok((out, stats))
    }
}

/// ORs one matched-row bitmap into another (lane qs6-15). The longer of the two is kept and the
/// shorter ORed into it, so a merge allocates nothing: the bitmaps held are only the workers' own,
/// each charged to the statement's budget when the worker sized it. (A merge into the run's empty
/// partial used to allocate a copy of the first worker's bitmap that no budget saw.)
fn or_marks(into: &mut Vec<u64>, mut from: Vec<u64>) {
    if into.len() < from.len() {
        std::mem::swap(into, &mut from);
    }
    for (word, other) in into.iter_mut().zip(&from) {
        *word |= *other;
    }
}

/// [`or_marks`] for a chain's bitmaps, one per link (lane qs6-15).
fn or_link_marks(into: &mut Vec<Vec<u64>>, mut from: Vec<Vec<u64>>) {
    if into.len() < from.len() {
        std::mem::swap(into, &mut from);
    }
    for (marks, other) in into.iter_mut().zip(from) {
        or_marks(marks, other);
    }
}

/// One phase of a [`TableChain`] (lane qs6-14): the probe's leaves (`from` none), or the build
/// rows of link `from` that no row of the phases before matched (`matched`, merged), each padded
/// with NULLs for the tables before the link and walked through the links after it.
pub struct ChainPhase<'a> {
    pub chain: &'a TableChain<'a>,
    pub from: Option<usize>,
    pub matched: &'a [Vec<u64>],
}

impl<'a> ChainPhase<'a> {
    /// Walks the build rows `rows` of link `link` that no row of the phases before matched, each
    /// padded with NULLs for the tables before the link, through the links after it.
    fn tails(
        &self,
        walk: &mut ChainWalk<'a, '_>,
        link: usize,
        rows: Range<usize>,
        budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        let chain = self.chain;
        let stage = chain.stages.get(link).ok_or(MorselError::Overflow {
            what: "a join's link",
        })?;
        let base = chain.before(link)?;
        let marks = self
            .matched
            .get(link)
            .map(Vec::as_slice)
            .unwrap_or_default();
        // The rows padded once the bitmaps merged (lane qs6-15).
        let mut pads = 0u64;
        for at in rows {
            let (word, bit) = matched_bit(at)?;
            if marks.get(word).is_some_and(|w| w & bit != 0) {
                continue;
            }
            let build_row = stage.rows.get(at).ok_or(MorselError::Overflow {
                what: "an unmatched build row",
            })?;
            walk.joined.clear();
            walk.joined
                .try_reserve(base.saturating_add(build_row.len()))?;
            walk.joined.resize(base, Value::Null);
            walk.joined.extend_from_slice(build_row);
            pads = pads.saturating_add(1);
            chain.walk(walk, link.saturating_add(1), None, at, budget)?;
        }
        OuterCounters::add(&chain.counts.bitmap_pads, pads);
        Ok(())
    }
}

impl Pipeline for ChainPhase<'_> {
    type Partial = ChainPartial;

    fn rows(&self) -> usize {
        match self.from {
            None => self.chain.table.leaves(),
            Some(link) => self
                .chain
                .stages
                .get(link)
                .map_or(0, |stage| stage.rows.len()),
        }
    }

    fn empty(&self) -> Self::Partial {
        ChainPartial::default()
    }

    fn run(
        &self,
        rows: Range<usize>,
        partial: &mut Self::Partial,
        budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        let chain = self.chain;
        let ChainPartial {
            aggregate,
            runs,
            matched,
            worker,
        } = partial;
        let worker = match worker {
            Some(worker) => worker,
            None => worker.insert(Box::new(chain.body.worker()?)),
        };
        // The worker's bitmaps, a bit per build row of each link that keeps the right rows,
        // charged to the statement's budget.
        if matched.is_empty() {
            matched.try_reserve_exact(chain.links.len())?;
            for ((keeps, _), stage) in chain.links.iter().zip(chain.stages) {
                let words = if keeps.right {
                    stage.rows.len().div_ceil(64)
                } else {
                    0
                };
                budget.charge(words.saturating_mul(std::mem::size_of::<u64>()))?;
                let mut marks = Vec::new();
                marks.try_reserve_exact(words)?;
                marks.resize(words, 0);
                matched.push(marks);
                OuterCounters::add(
                    &chain.counts.bitmap_words,
                    u64::try_from(words).unwrap_or(u64::MAX),
                );
            }
        }
        let first = rows.start;
        let mut walk = ChainWalk {
            worker,
            aggregate,
            matched,
            found: Vec::new(),
            joined: Vec::new(),
            frames: Vec::new(),
            ran: 0,
            pads: 0,
        };
        let walked = match self.from {
            None => chain.table.scan(rows, |rowid, row| {
                for term in chain.probe_filter {
                    if !term.holds(row)? {
                        return Ok(());
                    }
                }
                walk.joined.clear();
                walk.joined.try_reserve(row.len())?;
                walk.joined.extend_from_slice(row);
                chain.walk(&mut walk, 0, Some(rowid), position(rowid)?, budget)
            }),
            Some(link) => self.tails(&mut walk, link, rows, budget),
        };
        let ChainWalk {
            mut found,
            ran,
            pads,
            ..
        } = walk;
        if walked.is_ok() {
            chain
                .body
                .ran
                .fetch_add(ran, std::sync::atomic::Ordering::Relaxed);
            OuterCounters::add(&chain.counts.walk_pads, pads);
        }
        walked?;
        if !chain.order.is_empty() {
            found.sort_by(|a, b| compare_rows(chain.order, a, b));
        }
        runs.try_reserve(1)?;
        runs.push((first, found));
        Ok(())
    }

    fn merge(
        &self,
        into: &mut Self::Partial,
        mut from: Self::Partial,
        budget: &MemoryBudget,
    ) -> Result<(), MorselError> {
        or_link_marks(&mut into.matched, std::mem::take(&mut from.matched));
        self.chain
            .body
            .shape
            .merge(&mut into.aggregate, from.aggregate, budget)?;
        into.runs.try_reserve(from.runs.len())?;
        into.runs.extend(from.runs);
        Ok(())
    }
}

/// Orders two output rows by the sort keys: each one's place in the row, and whether it descends.
fn compare_rows(order: &[(usize, bool)], a: &[Value], b: &[Value]) -> std::cmp::Ordering {
    for (at, descending) in order {
        let order = match (a.get(*at), b.get(*at)) {
            (Some(x), Some(y)) => x.cmp(y),
            _ => std::cmp::Ordering::Equal,
        };
        let order = if *descending { order.reverse() } else { order };
        if order.is_ne() {
            return order;
        }
    }
    std::cmp::Ordering::Equal
}

/// A copy of an output row the body kept, its values and their text and blob bytes charged to
/// the statement's budget.
fn kept(out: &[Value], budget: &MemoryBudget) -> Result<Vec<Value>, MorselError> {
    let bytes = out.iter().fold(
        std::mem::size_of::<Value>().saturating_mul(out.len()),
        |sum, value| {
            sum.saturating_add(match value {
                Value::Text(text) => text.as_str().len(),
                Value::Blob(blob) => blob.len(),
                _ => 0,
            })
        },
    );
    budget.charge(bytes)?;
    let mut row = Vec::new();
    row.try_reserve_exact(out.len())?;
    row.extend(out.iter().cloned());
    Ok(row)
}

#[cfg(test)]
#[path = "morsel_scan_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "morsel_join_ab.rs"]
mod join_ab;
