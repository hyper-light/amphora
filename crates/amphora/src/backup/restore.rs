//! The shell that restores a second from the archive (docs/design/backup.md): the base's pages
//! written as the file, then each generation of log after it written as the log's file and
//! replayed by the engine's own recovery, the last one up to the second's last commit.

use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use amphora_api::blobs::Blobs;

use super::Mark;
use super::drill::{Pace, Unpaced};
use super::object::{self, SegmentHead};
use super::plan::{self, BaseKey, SegmentKey, ThroughKey, Unrestorable};
use super::ship::{FRAME_BYTES, fetch_charge, frame_offset, list_all};
use crate::db::{Db, log_paths};
use crate::memory::{Budget as Memory, Charge, Use};

/// What a restore did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored {
    /// The generation and frame of the last commit it kept.
    pub cut: (u64, u64),
    pub base: u64,
    pub bytes: u64,
    /// The bytes of log it fetched ([`super::check_due`]'s measure).
    pub log_bytes: u64,
}

/// The through marks under `prefix` ([`plan::ThroughKey`]).
pub(super) fn through_marks(
    blobs: &dyn Blobs,
    prefix: &str,
    until: Instant,
) -> anyhow::Result<Vec<(ThroughKey, String)>> {
    let mut marks = Vec::new();
    for (key, _) in list_all(blobs, &format!("{prefix}/through/"), until)? {
        marks.push((ThroughKey::parse(prefix, &key)?, key));
    }
    Ok(marks)
}

/// Restores the database as it stood at the end of `second` (every commit stamped at or before
/// it) from the archive under `prefix`, into a new file at `path`.
pub fn restore(
    blobs: &dyn Blobs,
    prefix: &str,
    second: u64,
    path: &Path,
    until: Instant,
) -> anyhow::Result<Restored> {
    restore_paced(blobs, prefix, second, path, &mut Unpaced, until)
}

/// [`restore`], each base chunk, log segment and replay a step of `pace`'s (T-T1.12.C7).
pub fn restore_paced(
    blobs: &dyn Blobs,
    prefix: &str,
    second: u64,
    path: &Path,
    pace: &mut dyn Pace,
    until: Instant,
) -> anyhow::Result<Restored> {
    restore_within(
        blobs,
        prefix,
        second,
        path,
        pace,
        &crate::memory::process(),
        until,
    )
}

/// What a restore hands each segment's frames to: its head, the frame's offset, and its bytes.
type Frames<'a> = dyn FnMut(&SegmentHead, u64, &[u8]) -> anyhow::Result<()> + 'a;

/// Reads the sealed segment at `key`, `size` bytes, in ranged gets of `part` bytes at most, each
/// held and charged to `memory` while it's read (T-T1.12.M3): its head ([`SegmentHead`]) kept, and
/// charged, as it comes in, then each run of its frames' bytes given to `frames` with its offset
/// in the frames, in order, held no longer than its part. Its sum is checked once the last part is
/// in: what `frames` was given before is a failed read's.
pub(super) fn read_segment(
    blobs: &dyn Blobs,
    key: &str,
    size: u64,
    part: usize,
    memory: &Arc<Memory>,
    until: Instant,
    frames: &mut Frames<'_>,
) -> anyhow::Result<SegmentHead> {
    if part == 0 {
        return Err(Unrestorable::SegmentPastRoom { key: key.into() }.into());
    }
    let malformed = |why: String| Unrestorable::Malformed {
        key: key.into(),
        why,
    };
    let mut opening = object::Opening::default();
    let mut head: Option<SegmentHead> = None;
    let mut pending: Vec<u8> = Vec::new();
    let mut pending_held: Vec<Charge> = Vec::new();
    let mut frames_at = 0u64;
    let mut at = 0u64;
    while at < size {
        let want = usize::try_from(size.saturating_sub(at))
            .unwrap_or(usize::MAX)
            .min(part);
        let _held = memory.take(Use::Chunks, u64::try_from(want)?)?;
        let got = blobs.get_range(key, at, want, until)?;
        if got.is_empty() {
            anyhow::bail!("archive object {key} ends at byte {at}, before its {size}");
        }
        at = at.saturating_add(u64::try_from(got.len())?);
        let body = opening.feed(&got);
        if let Some(h) = &head {
            frames(h, frames_at, body)?;
            frames_at = frames_at.saturating_add(u64::try_from(body.len())?);
            continue;
        }
        // The head: decoded from the part where it ends there, else kept across parts, and
        // charged while it's kept, until it decodes.
        let decoded = if pending.is_empty() {
            postcard::take_from_bytes::<SegmentHead>(body)
        } else {
            pending_held.push(memory.take(Use::Chunks, u64::try_from(body.len())?)?);
            pending.extend_from_slice(body);
            postcard::take_from_bytes::<SegmentHead>(&pending)
        };
        match decoded {
            Ok((h, rest)) => {
                if h.format != object::FORMAT {
                    return Err(malformed(format!("a segment of format {}", h.format)).into());
                }
                if !rest.is_empty() {
                    frames(&h, 0, rest)?;
                }
                frames_at = u64::try_from(rest.len())?;
                head = Some(h);
            }
            Err(postcard::Error::DeserializeUnexpectedEnd) => {
                if pending.is_empty() {
                    pending_held.push(memory.take(Use::Chunks, u64::try_from(body.len())?)?);
                    pending.extend_from_slice(body);
                }
                continue;
            }
            Err(e) => return Err(malformed(format!("its head doesn't decode: {e}")).into()),
        }
        pending = Vec::new();
        pending_held.clear();
    }
    opening.finish(key)?;
    let head = head.ok_or_else(|| malformed("it ends within its head".into()))?;
    if frames_at != head.frames {
        return Err(malformed(format!(
            "it holds {frames_at} bytes of frames, its length says {}",
            head.frames
        ))
        .into());
    }
    Ok(head)
}

/// [`restore_paced`], its chunks charged to `memory` (the process's budget, or a test's).
///
/// A chunk read in parts (T-T1.12.M2): the restore opens a stream of [`Use::Chunks`] and fetches
/// each of the base's chunks in ranged gets ([`Blobs::get_range`]) of its stream's share of the
/// budget's room, no more than the store's objects nor the largest sealed chunk the manifest
/// records ([`fetch_charge`]), one part held and charged at a time, its sum checked over the parts
/// ([`object::Opening`]). So a restore with less room than the backup had fetches in more, smaller
/// requests; with no room for a part it's refused, typed ([`Unrestorable::PastRoom`]), before it
/// makes the file. A chunk whose sum fails is refused after its parts are written: the restored
/// file is a failed restore's.
///
/// An archive short of the frames its keys say is refused, typed ([`Unrestorable::Short`]), and the
/// files made are removed: they would open as a whole database (AC-T1.12.4).
pub fn restore_within(
    blobs: &dyn Blobs,
    prefix: &str,
    second: u64,
    path: &Path,
    pace: &mut dyn Pace,
    memory: &Arc<Memory>,
    until: Instant,
) -> anyhow::Result<Restored> {
    let refused = match make(blobs, prefix, second, path, pace, memory, until) {
        Err(e) if matches!(e.downcast_ref(), Some(Unrestorable::Short { .. })) => e,
        restored => return restored,
    };
    let [log_path, second_path] = log_paths(path);
    for file in [path, &log_path, &second_path] {
        match std::fs::remove_file(file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(refused.context(format!("{} is left: {e}", file.display())));
            }
            _ => {}
        }
    }
    Err(refused)
}

/// [`restore_within`]'s restore, its files left as it ends.
fn make(
    blobs: &dyn Blobs,
    prefix: &str,
    second: u64,
    path: &Path,
    pace: &mut dyn Pace,
    memory: &Arc<Memory>,
    until: Instant,
) -> anyhow::Result<Restored> {
    // Nor a segment of a log left there (decision 45): the engine would recover it as the restored
    // log's own.
    let [log_path, second_path] = log_paths(path);
    for file in [path, &log_path, &second_path] {
        if file.try_exists()? {
            anyhow::bail!("{} exists: a restore makes a new database", file.display());
        }
    }
    let mut listed = Vec::new();
    for (key, size) in list_all(blobs, &format!("{prefix}/log/"), until)? {
        listed.push((SegmentKey::parse(prefix, &key)?, key, size));
    }
    // The log once over (T-T1.12.M6): where a retried send left segments over frames others
    // hold, those a kept one covers are left out.
    let parsed: Vec<SegmentKey> = listed.iter().map(|(s, _, _)| s.clone()).collect();
    let keys: Vec<(SegmentKey, String, u64)> = plan::cover(&parsed)
        .into_iter()
        .filter_map(|i| listed.get(i).cloned())
        .collect();
    drop(listed);
    let mut segments: Vec<SegmentKey> = keys.iter().map(|(s, _, _)| s.clone()).collect();
    let marks: Vec<ThroughKey> = through_marks(blobs, prefix, until)?
        .into_iter()
        .map(|(m, _)| m)
        .collect();
    plan::raise_through(&mut segments, &marks);
    let mut bases = Vec::new();
    for (key, _) in list_all(blobs, &format!("{prefix}/base/"), until)? {
        bases.push(BaseKey::parse(prefix, &key)?);
    }
    let at = plan::locate(&segments, second)?;
    let (at_key, at_size) = keys
        .get(at)
        .map(|(_, k, size)| (k.clone(), *size))
        .ok_or_else(|| anyhow::anyhow!("segment {at} is gone"))?;
    // One stream of the restore's for the whole of it: a segment is read in parts of half its
    // share (a part and the head of a segment kept while it decodes, [`read_segment`]), as the
    // base's chunks are read in parts of its share (T-T1.12.M3).
    let stream = memory.stream(Use::Chunks);
    let room = memory
        .stream_room(Use::Chunks)?
        .min(u64::try_from(blobs.object_max())?);
    let log_part = usize::try_from(room.checked_div(2).unwrap_or(0))?;
    // The marks only: its frames are read again where the log is written.
    let located = read_segment(
        blobs,
        &at_key,
        at_size,
        log_part,
        memory,
        until,
        &mut |_, _, _| Ok(()),
    )?;
    let marks: Vec<Mark> = located
        .marks
        .iter()
        .map(|m| Mark {
            seq: located.identity.seq,
            frame: m.frame,
            ns: m.ns,
            unknown_from: m.unknown_from,
        })
        .collect();
    let frame = plan::cut_in(&marks, second)?;
    let plan = plan::chain(&segments, &bases, at, frame, second)?;
    let mut bytes = 0u64;
    let mut log_bytes = 0u64;

    // The base's pages, as the file.
    let manifest_key = plan.base.key(prefix);
    let manifest = object::manifest(&manifest_key, &blobs.get(&manifest_key, until)?)?;
    let part = room.min(fetch_charge(manifest.chunk_bytes.min(manifest.file_bytes)));
    if part == 0 {
        return Err(Unrestorable::PastRoom { base: manifest.id }.into());
    }
    let held = memory.take(Use::Chunks, part)?;
    let part = usize::try_from(part)?;
    {
        let mut file = std::fs::File::create_new(path)?;
        // An incremental base keeps an earlier base's chunks where the log changed none of their
        // pages (docs/design/backup.md, "Incremental bases").
        for (chunk, base) in (0u64..).zip(manifest.chunk_bases()?) {
            let key = plan::chunk_key(prefix, base, chunk);
            pace.ready()?;
            let len = manifest.chunk_bytes.min(
                manifest
                    .file_bytes
                    .saturating_sub(chunk.saturating_mul(manifest.chunk_bytes)),
            );
            let sealed = fetch_charge(len);
            let mut opening = object::Opening::default();
            let mut at = 0u64;
            while at < sealed {
                let want = usize::try_from(sealed.saturating_sub(at))
                    .unwrap_or(usize::MAX)
                    .min(part);
                let got = blobs.get_range(&key, at, want, until)?;
                if got.is_empty() {
                    anyhow::bail!("archive object {key} ends at byte {at}, before its {sealed}");
                }
                file.write_all(opening.feed(&got))?;
                at = at.saturating_add(u64::try_from(got.len())?);
            }
            opening.finish(&key)?;
            pace.done(Some(&file), len)?;
            bytes = bytes.saturating_add(sealed);
        }
        file.sync_all()?;
    }
    drop(held);

    // Each generation's log, replayed whole but the last, which ends at the cut.
    let mut index = 0usize;
    while let Some(first) = plan.segments.get(index) {
        let generation = segments.get(*first).map_or(0, |s| s.generation);
        let short = |archived: u64, replayed: u64| Unrestorable::Short {
            generation,
            archived,
            replayed,
        };
        // The first generation's log is new (checked above); each later one takes the place of the
        // log the fold before it emptied but left (a truncating checkpoint keeps its file).
        let mut log = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&log_path)?;
        // The log as written, read back where a segment overlaps the one before it.
        let mut written_log = std::fs::File::open(&log_path)?;
        let mut written = 0u64;
        let mut header_written = false;
        while let Some(i) = plan.segments.get(index) {
            let Some((key, name, size)) = keys.get(*i) else {
                anyhow::bail!("segment {i} is gone");
            };
            if key.generation != generation {
                break;
            }
            index = index.saturating_add(1);
            pace.ready()?;
            let end = if generation == plan.cut.0 {
                key.last.min(plan.cut.1)
            } else {
                key.last
            };
            if key.first > written.saturating_add(1) {
                return Err(short(end, written).into());
            }
            // Its frames' bytes that are in the log already, from the segment before, which this
            // one begins within (a send retried, [`plan::cover`]), and those up to the cut.
            let begins = key.first;
            let overlap = written
                .saturating_add(1)
                .saturating_sub(begins)
                .saturating_mul(FRAME_BYTES);
            let upto = end
                .saturating_add(1)
                .saturating_sub(begins)
                .saturating_mul(FRAME_BYTES);
            let take = upto.saturating_sub(overlap);
            let at_log = frame_offset(begins.saturating_sub(1))?;
            // Its frames up to the cut written as their parts come (T-T1.12.M3), after the log's
            // header where this is the generation's first segment; those the log holds already
            // compared with it, a frame at a time, and refused where they differ (T-T1.12.M6).
            let mut put = |head: &SegmentHead, offset: u64, part: &[u8]| -> anyhow::Result<()> {
                if offset == 0 {
                    if head.first != begins {
                        anyhow::bail!(
                            "segment {name} begins at frame {}, its key says {begins}",
                            head.first
                        );
                    }
                    if !header_written {
                        log.write_all(&head.header)?;
                        header_written = true;
                    }
                }
                let to = offset.saturating_add(u64::try_from(part.len())?);
                let mut at = offset;
                while at < to.min(overlap) {
                    let n = to.min(overlap).saturating_sub(at).min(FRAME_BYTES);
                    let _held = memory.take(Use::Chunks, n)?;
                    let mut ours = vec![0u8; usize::try_from(n)?];
                    written_log.seek(SeekFrom::Start(at_log.saturating_add(at)))?;
                    written_log.read_exact(&mut ours)?;
                    let within = usize::try_from(at.saturating_sub(offset))?;
                    let theirs = part.get(within..within.saturating_add(ours.len()));
                    if theirs != Some(ours.as_slice()) {
                        anyhow::bail!(
                            "segment {name} overlaps the log before it and differs from it at its byte {at}"
                        );
                    }
                    at = at.saturating_add(n);
                }
                let from = usize::try_from(overlap.saturating_sub(offset))
                    .unwrap_or(usize::MAX)
                    .min(part.len());
                let keep = usize::try_from(upto.saturating_sub(offset))
                    .unwrap_or(usize::MAX)
                    .min(part.len());
                if from < keep {
                    log.write_all(part.get(from..keep).unwrap_or_default())?;
                }
                Ok(())
            };
            let head = read_segment(blobs, name, *size, log_part, memory, until, &mut put)?;
            bytes = bytes.saturating_add(*size);
            log_bytes = log_bytes.saturating_add(if *i == at { head.frames } else { *size });
            if head.first != begins {
                anyhow::bail!("segment {name} doesn't follow frame {written}");
            }
            if head.frames < upto {
                let held = head.frames.checked_div(FRAME_BYTES).unwrap_or(0);
                return Err(short(end, begins.saturating_sub(1).saturating_add(held)).into());
            }
            if !header_written {
                log.write_all(&head.header)?;
                header_written = true;
            }
            pace.done(Some(&log), take)?;
            written = end;
            if generation == plan.cut.0 && written >= plan.cut.1 {
                break;
            }
        }
        log.sync_all()?;
        drop(log);
        let replayed = replay(path, written, super::check::step_bytes(blobs), pace)?;
        if replayed != written {
            return Err(short(written, replayed).into());
        }
        if generation == plan.cut.0 {
            if written != plan.cut.1 {
                return Err(short(plan.cut.1, written).into());
            }
            break;
        }
    }
    drop(stream);
    Ok(Restored {
        cut: plan.cut,
        base: plan.base.id,
        bytes,
        log_bytes,
    })
}

/// Folds the restored log's `frames` into the file. Unpaced, in one step, as the engine folds a log.
/// Paced, in parts of as many frames as one of the store's objects holds (`step` bytes, the size a
/// restore's other steps move), each part a step made durable as it ends, so a paced check pauses
/// inside the replay (T-T1.12.C8); the last fold empties the log.
///
/// The frames it replayed: fewer than `frames`, none folded, where the log isn't what the archive
/// says. The engine's recovery keeps the frames that follow one another from the log's header (their
/// salts and running sums), up to their last commit, and drops the rest as a crash's tail; a
/// segment's own sum says its bytes are those sent, not that they are its generation's.
fn replay(path: &Path, frames: u64, step: u64, pace: &mut dyn Pace) -> anyhow::Result<u64> {
    let mut db = Db::open(path)?;
    let (_, replayed) = db.log_end();
    if replayed != frames {
        return Ok(replayed);
    }
    if pace.paced() {
        let file = std::fs::OpenOptions::new().write(true).open(path)?;
        let per_step = step.checked_div(FRAME_BYTES).unwrap_or(0).max(1);
        let mut through = 0u64;
        while through < frames {
            pace.ready()?;
            let to = through.saturating_add(per_step).min(frames);
            db.fold_log_through(to)?;
            pace.done(
                Some(&file),
                to.saturating_sub(through).saturating_mul(FRAME_BYTES),
            )?;
            through = to;
        }
        pace.ready()?;
    }
    db.fold_log()?;
    drop(db);
    pace.done(None, 0)?;
    Ok(replayed)
}

/// Deletes what a retention of `retention_ns` lets go at `now_ns` ([`plan::expired`]): how many
/// objects.
pub fn expire(
    blobs: &dyn Blobs,
    prefix: &str,
    now_ns: u64,
    retention_ns: u64,
    until: Instant,
) -> anyhow::Result<usize> {
    let mut segments = Vec::new();
    for (key, _) in list_all(blobs, &format!("{prefix}/log/"), until)? {
        segments.push(SegmentKey::parse(prefix, &key)?);
    }
    let marks: Vec<ThroughKey> = through_marks(blobs, prefix, until)?
        .into_iter()
        .map(|(m, _)| m)
        .collect();
    plan::raise_through(&mut segments, &marks);
    let mut bases = Vec::new();
    for (key, _) in list_all(blobs, &format!("{prefix}/base/"), until)? {
        bases.push(BaseKey::parse(prefix, &key)?);
    }
    let mut chunks = Vec::new();
    for (key, _) in list_all(blobs, &format!("{prefix}/pages/"), until)? {
        chunks.push((plan::chunk_id(prefix, &key)?, key));
    }
    let mut gone = plan::expired(prefix, &segments, &bases, &chunks, now_ns, retention_ns);
    // A chunk a kept base reuses stays, whichever base first stored it.
    let mut reused = std::collections::BTreeSet::new();
    for base in &bases {
        let key = base.key(prefix);
        if gone.contains(&key) {
            continue;
        }
        let manifest = object::manifest(&key, &blobs.get(&key, until)?)?;
        for (chunk, from) in (0u64..).zip(manifest.chunk_bases()?) {
            if from != manifest.id {
                reused.insert(plan::chunk_key(prefix, from, chunk));
            }
        }
    }
    gone.retain(|key| !reused.contains(key));
    for key in &gone {
        blobs.delete(key, until)?;
    }
    Ok(gone.len())
}
