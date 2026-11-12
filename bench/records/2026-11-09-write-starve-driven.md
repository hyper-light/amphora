# The follower-write prototypes on the driven harness: isolation checked (2026-11-09)

Step 4 of kiln plan 01a1b8d174dc72d18b9d2289b6e8fe11 (goal 9c63f7d4; docs/research/follower-write-starvation.md, f4,
f6, f7, f21). The candidates built for real behind one switch (`replication::admission::Admission`, today's exact base
its default) and run on the driven harness: three real owners, their real engines and logs, every relayed record
delayed a seeded 0 to 3 relay rounds. A correctness record, not a timing: rounds are the harness's, not a machine's.

- **Host:** adas-mac-studio.local-2 (runner d1323898), debug test profile, `cargo test --locked -p amphora --lib
  every_prototype_admission -- --nocapture`, commit ec337ba, 2026-11-09, 180 s.
- **What runs:** `database::replicated::tests::every_prototype_admission_keeps_list_append_histories_serializable`. The
  leader and one follower each make 15 list-append writes, one at a time; each reads two lists and appends a unique
  element to one; a write refused as stale is made again as a new write (kiln's `store::write`). Two hot lists, then 32
  lists each padded to half a page (so lists far apart sit on different pages); seeds 1 to 3.
- **The checks** (Elle's list-append, f21): every replica holds the same lists; each committed element appears once,
  in its own list, and no refused one does; each committed write read its list exactly as it stood before its element
  (the serial replay of each list) and its other list as a prefix; and Elle's dependency graph (ww from each list's
  order, wr from what was read, rw to the element a read missed) has no cycle (no G0, G1c or G2).

| candidate | lists | violations | leader's writes committed / refused | follower's committed / refused, by seed | rounds, by seed |
|---|---|---|---|---|---|
| A0 Exact (today) | 2 | 0 | 15 / 10, 7, 7 | 15 / 15, 14, 14 | 1310, 1187, 1199 |
| A0 Exact (today) | 32 | 0 | 15 / 9, 7, 6 | 15 / 14, 14, 12 | 1192, 1186, 1107 |
| B, the follower's write run at the leader | 2 | 0 | 15 / 0 | 15 / 0, 0, 0 | 735, 753, 765 |
| B, the follower's write run at the leader | 32 | 0 | 15 / 0 | 15 / 0, 0, 0 | 757, 760, 761 |
| A1m Pages | 2 | 0 | 15 / 0 | 15 / 10, 10, 10 | 1043, 1025, 1016 |
| A1m Pages | 32 | 0 | 15 / 0 | 15 / 2, 2, 2 | 720, 721, 697 |
| D PagesThenSlot | 2 | 0 | 15 / 0 | 15 / 3, 3, 4 | 799, 829, 834 |
| D PagesThenSlot | 32 | 0 | 15 / 0 | 15 / 1, 0, 0 | 725, 713, 714 |
| G ExactThenSlot | 2 | 0 | 15 / 0 | 15 / 5, 4, 4 | 868, 849, 850 |
| G ExactThenSlot | 32 | 0 | 15 / 0 | 15 / 7, 3, 5 | 934, 808, 935 |
| C Slot | 2 | 0 | 15 / 0 | 15 / 0, 0, 0 | 684, 671, 665 |
| C Slot | 32 | 0 | 15 / 0 | 15 / 0, 0, 0 | 662, 659, 662 |
| mutant: validation skipped | 2 | 10, 10, 9 | | 15 / | |

What it shows:
- **No candidate admitted a non-serializable history** in any seed; the mutant that applies any base was caught in
  every seed (lost appends: committed elements missing from their list, and reads that don't match the replay).
- **Today's rule refuses the follower about once per write even here** (12 to 15 refusals for 15 writes), and refuses
  the leader's own writes too (a follower's refused entry moves the point under the leader's preparation).
- **Page validation (A1m) helps where the pages differ** (2 refusals over 32 spread lists) **and barely on hot ones** (10
  of 15 on two lists on one page), as the model said. **Its hybrids D and G cut the follower's refusals to 0 to 7**, and
  **C and B to none**. The leader was never refused under any candidate but today's.
- B here runs the follower's session at the leader's owner from the test, the session standing for a named command
  both members run: it has no wire path (a command entry, a leader-side queue and the result's way back to the
  follower are not built), so these rounds don't count the forward's hop.

Not built, and why: C's slot holds the leader for a follower's every ReadIndex, reads too, until that member's next
write applies or 200 ms pass (`SLOT`). Under the replicated class a follower answers on the commit by today's exact rule
(`replication::verdict`); the page admissions decide at apply, so these prototypes run in the persisted class, where a
follower answers at its apply.
