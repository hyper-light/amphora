//! COPY FROM STDIN on an engine connection (REL_19_0 src/backend/commands/copyfrom.c and copyfromparse.c): the rows the
//! client sends in CopyData messages inserted in the statement's transaction, and COPY's end by CopyDone or CopyFail.
//! Rows are converted as they come and inserted in 19's batches, so of several refusals in one piece of data the first
//! is the one 19 raises.

use amphora_engine::dialect::postgres::to_engine;
use amphora_engine::{LimboError, Statement};
use amphora_pg_analysis::Value;
use amphora_pg_analysis::copy::{CopyIn as Spec, Lines, Read, Refused, copy_in};
use amphora_pg_parser::tree::{NodeId, Tree};
use std::num::NonZero;

use super::execute::{Block, Executor, Ran, Stop, complete, engine, refusal};
use super::session::{Copy, Flow};
use super::wire::{self, PgError, SqlState};
use crate::memory::Charge;

/// `MAX_BUFFERED_TUPLES` and `MAX_BUFFERED_BYTES` (copyfrom.c): how much a table's rows wait to be inserted.
const BATCH_ROWS: usize = 1000;
const BATCH_BYTES: usize = 65_535;

/// A COPY under way.
pub(super) struct CopyIn {
    spec: Spec,
    lines: Lines,
    insert: Statement,
    rows: Vec<(u64, Vec<Option<Value>>)>,
    bytes: usize,
    copied: u64,
    /// Whether its transaction began with it: it ends with it.
    own: bool,
    charged: usize,
    held: Vec<Charge>,
}

/// What goes on once a COPY has ended.
pub(super) enum Then {
    Query(super::execute::Query),
    /// An Execute, whose answer no ReadyForQuery ends.
    Execute,
}

impl Executor {
    /// `BeginCopyFrom` and the start of `CopyFrom`: the statement's transaction, CopyInResponse, and what FREEZE needs.
    pub(super) fn copy_begin(
        &mut self,
        sql: &str,
        tree: &Tree,
        id: NodeId,
        out: &mut Vec<u8>,
    ) -> Result<Box<CopyIn>, PgError> {
        let spec = {
            let session = self.analysis(None);
            copy_in(tree, id, &session)
                .map_err(|e| refusal(e, sql))?
                .ok_or_else(|| PgError::error(SqlState::INTERNAL_ERROR, "not a COPY"))?
        };
        let own = self.block == Block::None;
        if own {
            self.begin()?;
        } else {
            self.ensure_transaction()?;
        }
        let started = self.copy_started(&spec, own, out);
        if started.is_err() && own {
            // The transaction it began goes with it.
            self.end();
        }
        started
    }

    fn copy_started(
        &mut self,
        spec: &Spec,
        own: bool,
        out: &mut Vec<u8>,
    ) -> Result<Box<CopyIn>, PgError> {
        let names: Vec<String> = spec
            .columns
            .iter()
            .map(|column| format!("\"{}\"", column.name.replace('"', "\"\"")))
            .collect();
        let binds: Vec<String> = (1..=spec.columns.len()).map(|n| format!("?{n}")).collect();
        let text = format!(
            "INSERT INTO \"{}\"({}) VALUES ({})",
            spec.relname.replace('"', "\"\""),
            names.join(", "),
            binds.join(", ")
        );
        let insert = self.prepare(&text)?;
        // ReceiveCopyBegin: the overall format and each column's, text.
        let count = i16::try_from(spec.fields.len())
            .map_err(|_| PgError::error(SqlState::INTERNAL_ERROR, "a COPY has too many columns"))?;
        wire::message(out, b'G', |out| {
            out.push(0);
            out.extend_from_slice(&count.to_be_bytes());
            for _ in 0..count {
                out.extend_from_slice(&0i16.to_be_bytes());
            }
        })?;
        if spec.freeze {
            if self.holds_a_ready_portal() {
                return Err(PgError::error(
                    SqlState::new("25000"),
                    "cannot perform COPY FREEZE because of prior transaction activity",
                ));
            }
            if !self.made_or_emptied(spec.relation) {
                return Err(PgError::error(
                    SqlState::OBJECT_NOT_IN_PREREQUISITE_STATE,
                    "cannot perform COPY FREEZE because the table was not created or truncated in the current subtransaction",
                ));
            }
        }
        Ok(Box::new(CopyIn {
            spec: spec.clone(),
            lines: Lines::default(),
            insert,
            rows: Vec::new(),
            bytes: 0,
            copied: 0,
            own,
            charged: 0,
            held: Vec::new(),
        }))
    }

    /// A message of a COPY under way: [`Flow::CopyIn`] while it goes on.
    pub(super) fn copy_message(
        &mut self,
        message: Copy<'_>,
        out: &mut Vec<u8>,
    ) -> Result<Flow, PgError> {
        let Some((mut copy, then)) = self.copying.take() else {
            return Ok(Flow::Done);
        };
        let ended = match message {
            Copy::Data(data) => {
                if let Err(error) = self.copy_data(&mut copy, data) {
                    Err(error)
                } else {
                    self.copying = Some((copy, then));
                    return Ok(Flow::CopyIn);
                }
            }
            // The line under way is the one after the last read.
            Copy::Fail(reason) => Err(PgError::error(
                SqlState::new("57014"),
                format!("COPY from stdin failed: {reason}"),
            )
            .with_context(copy.spec.at_line(copy.lines.number().saturating_add(1)))),
            Copy::Done => self.copy_done(&mut copy, out),
        };
        match (ended, then) {
            (Ok(()), Then::Query(mut query)) => {
                query.at = query.at.saturating_add(1);
                self.run_query(query, None, out)
            }
            (Ok(()), Then::Execute) => {
                self.pipelining = true;
                Ok(Flow::Done)
            }
            (Err(error), then) => {
                // The transaction it began goes with it, as any refusal's does.
                if copy.own {
                    self.end();
                }
                drop(copy);
                self.refused(&error, out)?;
                Ok(match then {
                    Then::Query(_) => Flow::Done,
                    Then::Execute => Flow::Refused,
                })
            }
        }
    }

    /// CopyData: its lines converted, and inserted in batches.
    fn copy_data(&mut self, copy: &mut CopyIn, data: &[u8]) -> Result<(), PgError> {
        let wanted = copy.lines.held().saturating_add(data.len());
        if wanted > copy.charged {
            let more = wanted.saturating_sub(copy.charged).max(copy.charged);
            if let Some(charge) = self.charge(more)? {
                copy.held.push(charge);
            }
            copy.charged = copy.charged.saturating_add(more);
        }
        copy.lines.push(data);
        self.copy_lines(copy, false)
    }

    /// The lines there are, to the data's end when `eof`.
    fn copy_lines(&mut self, copy: &mut CopyIn, eof: bool) -> Result<(), PgError> {
        loop {
            let read = copy.lines.next_line(eof);
            let number = copy.lines.number();
            match read {
                Ok(Read::More) => return Ok(()),
                Ok(Read::End) => return self.copy_flush(copy),
                Ok(Read::Line(line)) => {
                    let row = {
                        let session = self.analysis(Some(self.began));
                        copy.spec.row(number, &line, &session)
                    }
                    .map_err(|Refused { error, context }| {
                        refusal(error, "").with_context(context)
                    })?;
                    copy.bytes = copy.bytes.saturating_add(line.len());
                    copy.rows.push((number, row));
                    if copy.rows.len() >= BATCH_ROWS || copy.bytes >= BATCH_BYTES {
                        self.copy_flush(copy)?;
                    }
                }
                Err(error) => {
                    return Err(refusal(error, "").with_context(copy.spec.at_line(number)));
                }
            }
        }
    }

    /// `CopyMultiInsertBufferFlush`: the rows converted so far, inserted.
    fn copy_flush(&mut self, copy: &mut CopyIn) -> Result<(), PgError> {
        let rows = std::mem::take(&mut copy.rows);
        copy.bytes = 0;
        for (number, row) in rows {
            let context = copy.spec.at_line(number);
            self.copy_insert(copy, &row)
                .map_err(|error| error.with_context(context))?;
            copy.copied = copy.copied.saturating_add(1);
        }
        Ok(())
    }

    fn copy_insert(&mut self, copy: &mut CopyIn, row: &[Option<Value>]) -> Result<(), PgError> {
        copy.insert.reset().map_err(engine)?;
        for (at, (column, value)) in copy.spec.columns.iter().zip(row).enumerate() {
            let stored = match value {
                None => amphora_engine::Value::Null,
                Some(value) => to_engine(column.type_oid, value).map_err(engine)?,
            };
            let index = at.checked_add(1).and_then(NonZero::new).ok_or_else(|| {
                PgError::error(SqlState::INTERNAL_ERROR, "a COPY has too many columns")
            })?;
            copy.insert.bind_at(index, stored).map_err(engine)?;
        }
        match self.step(&mut copy.insert) {
            Ok(Ran::Done(_)) => Ok(()),
            // COPY doesn't wait for another session's transaction here.
            Ok(Ran::Waits(_)) => Err(engine(LimboError::SerializationFailure)),
            Err(error) => Err(engine(error)),
        }
    }

    /// CopyDone: the rest of the rows, the transaction it began kept, and its command tag.
    fn copy_done(&mut self, copy: &mut CopyIn, out: &mut Vec<u8>) -> Result<(), PgError> {
        self.copy_lines(copy, true)?;
        // A COPY that read `\.` has read its last line already.
        self.copy_flush(copy)?;
        if copy.own {
            self.conn_commit()?;
        }
        complete(out, &format!("COPY {}", copy.copied))
    }
}

impl Stop {
    /// A COPY began: its data is to come.
    pub(super) fn copy(copy: Box<CopyIn>) -> Self {
        Self::Copy(copy)
    }
}
