//! The snapshot source both catch-up designs share (replication.md, "Decision (lane snapshots)",
//! unit B): it answers an ask for page ranges with the pages of one read view, the point that view
//! reads at, and where the ask's budget stopped it. The push and the pull differ only in how long
//! the view lasts ([`Views`]), so the A/B measures that alone. Unit B wires it to the transport's
//! ask; until then only its tests call it.
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "unit B wires the source to Request::Pages")
)]

use hyper_durable::Point;

use crate::db::{Db, Pages};

/// How long a source's read view lasts: the one difference between the A/B's arms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Views {
    /// The pull: a view per answer, ended with it.
    PerAnswer,
    /// The push: one view begun at the first answer and held until the source drops.
    Held,
}

/// One answer: pages read from one view, in the order asked.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Answer {
    /// The applied point the view reads at, with its term.
    pub(crate) at: Point,
    /// The group's configuration at that point, as the replica's marker keeps it.
    pub(crate) configuration: Vec<u8>,
    /// The database's pages at that point.
    pub(crate) count: u32,
    pub(crate) pages: Pages,
    /// The first page the budget left unsent, or none when every range was answered.
    pub(crate) next: Option<u32>,
}

/// A source of pages on a connection of its own to a member's database ([`Db::reader`]), so its
/// view never holds the member's writer.
pub(crate) struct Source {
    reader: Db,
    views: Views,
    open: bool,
}

impl Source {
    pub(crate) fn new(reader: Db, views: Views) -> Self {
        Self {
            reader,
            views,
            open: false,
        }
    }

    /// The pages of `ranges` (each `first..=last`, past the database's last page cut off), in
    /// order, their bytes no more than `budget` but for the first, so an answer always moves.
    pub(crate) fn answer(
        &mut self,
        ranges: &[std::ops::RangeInclusive<u32>],
        budget: usize,
    ) -> anyhow::Result<Answer> {
        if !self.open {
            self.reader.exec("BEGIN")?;
            self.open = true;
        }
        let answered = self.read(ranges, budget);
        if self.views == Views::PerAnswer || answered.is_err() {
            self.end()?;
        }
        answered
    }

    fn read(
        &self,
        ranges: &[std::ops::RangeInclusive<u32>],
        budget: usize,
    ) -> anyhow::Result<Answer> {
        let mut marked = None;
        self.reader.query(
            "SELECT applied, term, configuration FROM __amphora_replica WHERE id = 1",
            [],
            |row| {
                let index = row.get::<String>(0)?.parse()?;
                let term = row.get::<String>(1)?.parse()?;
                let configuration = row
                    .get_value(2)
                    .to_blob()
                    .ok_or_else(|| anyhow::anyhow!("membership is not a blob"))?
                    .to_vec();
                marked = Some((Point { index, term }, configuration));
                Ok(false)
            },
        )?;
        let (at, configuration) =
            marked.ok_or_else(|| anyhow::anyhow!("the replica's marker is missing"))?;
        let count = self.reader.page_count()?;
        let (mut pages, mut bytes) = (Vec::new(), 0usize);
        for range in ranges {
            for id in *range.start()..=(*range.end()).min(count) {
                let page = self.reader.page(i64::from(id))?;
                let after = bytes
                    .checked_add(page.len())
                    .ok_or_else(|| anyhow::anyhow!("answer size overflow"))?;
                if after > budget && !pages.is_empty() {
                    return Ok(Answer {
                        at,
                        configuration,
                        count,
                        pages,
                        next: Some(id),
                    });
                }
                bytes = after;
                pages.push((id, page));
            }
        }
        Ok(Answer {
            at,
            configuration,
            count,
            pages,
            next: None,
        })
    }

    fn end(&mut self) -> anyhow::Result<()> {
        if !self.open {
            return Ok(());
        }
        self.open = false;
        self.reader.exec("ROLLBACK")
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        // A view that can't be ended now ends with the connection, which drops with this.
        let _ = self.end();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked(db: &Db, index: u64) {
        db.exec(&format!(
            "UPDATE __amphora_replica SET applied = '{index}' WHERE id = 1"
        ))
        .unwrap();
    }

    /// Lane snapshots, unit B's seam (replication.md, "Decision (lane snapshots)"): the push holds
    /// one view across its answers, the pull reads each answer from a view of its own. Do: a
    /// database with the replica's marker at 5 and rows; a held source and a per-answer source each
    /// asked for every page with a budget of two pages; a row written and the marker moved to 6; both
    /// asked for every page again. Expect: the first answers alike, at 5, two pages each, the next
    /// page named; after the write, the held source's answer still at 5 with the same pages, the
    /// per-answer source's at 6 with the database's new pages; the writer never held back.
    #[test]
    fn snapshots_a_held_view_answers_from_its_point_and_a_per_answer_view_from_now() {
        let dir = tempfile::tempdir().unwrap();
        let writer = Db::open(&dir.path().join("source.db")).unwrap();
        writer
            .exec("CREATE TABLE __amphora_replica (id INTEGER PRIMARY KEY, applied TEXT NOT NULL, term TEXT NOT NULL, configuration BLOB NOT NULL)")
            .unwrap();
        writer
            .exec("INSERT INTO __amphora_replica VALUES (1, '5', '2', x'0102')")
            .unwrap();
        writer.exec("CREATE TABLE rows(b)").unwrap();
        for _ in 0..4 {
            writer
                .exec("INSERT INTO rows VALUES (randomblob(3000))")
                .unwrap();
        }
        let mut each = Source::new(writer.reader().unwrap(), Views::PerAnswer);
        let page = each.answer(&[1..=1], 0).unwrap().pages[0].1.len();
        let mut held = Source::new(writer.reader().unwrap(), Views::Held);
        let first = held.answer(&[1..=u32::MAX], 2 * page).unwrap();
        assert_eq!(each.answer(&[1..=u32::MAX], 2 * page).unwrap(), first);
        assert_eq!(first.at, Point { index: 5, term: 2 });
        assert_eq!(first.configuration, vec![1, 2]);
        assert_eq!(first.pages.len(), 2, "a budget of two pages");
        assert_eq!(first.next, Some(3));
        assert!(first.count >= 3, "{} pages", first.count);

        for _ in 0..4 {
            writer
                .exec("INSERT INTO rows VALUES (randomblob(3000))")
                .unwrap();
        }
        marked(&writer, 6);
        let all = usize::MAX;
        let kept = held.answer(&[1..=u32::MAX], all).unwrap();
        let now = each.answer(&[1..=u32::MAX], all).unwrap();
        assert_eq!(kept.at, first.at, "the held view moved");
        assert_eq!((kept.count, kept.next), (first.count, None));
        assert_eq!(kept.pages[..2], first.pages[..], "the held view's pages changed");
        assert_eq!(now.at, Point { index: 6, term: 2 });
        assert!(now.count > first.count, "the per-answer view missed the writes");
        assert_eq!(now.pages.len(), usize::try_from(now.count).unwrap());
    }
}
