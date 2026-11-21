# Owner's record: what Codex decided, and where the owner departs

Written 2026-11-06 by amphora-seam-owner (kiln agent 01a1b152d389767493f79adff5245f64), which took Codex's role on
Ada's word ("Spin up a Claude session to handle it!"). Codex was kiln agent 01a188136cb1758cac52149a6eea8c7b, named
"Duplicates Fix", and it has ended. Sources: the Amphora agent's transcript
(~/.claude/projects/-home-adalundhe-Projects/01a14d4a-c6cb-76d2-a064-1029946ff226.jsonl), meaning the
`amphora-codex-*` and `amphora-reply-codex-*` messages it sent and the 26 messages Codex sent back. Extracts are in
~/.cache/claude-scratch/amphora/seam-owner-work/{codex-msgs.txt,codex-in.txt}.

Background: the Amphora agent wrote the code in Codex's files, and Codex reviewed it and composed it for kiln. Codex
seldom answered "agreed". Its usual way of accepting work was to compose it into kiln's private line. Where this
record says "accepted by composition", that is what happened.

## 1. Composition for kiln (where kiln's Amphora pin stands)
- e2e67247 was Codex's first fixed engine base (2026-10-30). It held d2d8d72, f998588, 33599f1, f36cc5b and f50cffe.
- Kiln's pin then moved to 327a8f9 (WITHOUT ROWID, on f61a395; seen 2026-10-31 12:01Z) and later to **4281466**
  (kiln C19 2c763e5, 2026-11-02 01:10Z, with Windows MSVC clippy passing there).
- The fixed-object bundle with heads 4281466, e6d079f and 29be700 was handed over on 2026-11-01. Its path is
  ~/.cache/claude-scratch/amphora/handoff/t1-4281466-e6d079f-29be700.bundle.
- Codex's rule: upstream gate evidence is recorded as upstream evidence and is never counted as kiln's gate. Codex
  composes fixed SHAs only, never a moving tree, and never pushes or merges.
- **Owner's position:** "composing onto e2e67247" no longer applies, because kiln is past it. A fix reaches kiln as a
  descendant of kiln's pin, through Amphora main, and Ada decides the merge and the repin.

## 2. replicated.rs: the owner's time (the flake fix). Accepted by composition (4281466 in kiln C4-C19)
- docs/design/replication.md, "The owner's time" (ff6f190, amended d5c7b2d): the actor reads time only from the Clock
  its starter supplies (`step_at`). Production uses the monotonic clock. Tests use the Driven shell, with a virtual
  clock per owner. The owner re-reads the clock after a preparation, and the SQL deadline reads the owner's clock
  (`Db::deadline_on`). Callers' own deadlines (submit, receive, try_result, start) stay on the real clock.
- Tests: no election runs during a scenario unless a nemesis forces one. The nemesis invariants are: never two Oks,
  never two rows, every answer is Ok, the stale refusal or UnknownOutcome, and the seed image is intact.
- Falsified by: a test shell outcome that differs from real threads, a term change without a nemesis, a verdict that
  rests on the real clock, or an owner that reads the real clock outside its shell.
- Left open by that decision: quorum patience taken from measured stalls. That is the disk-stalls work (§6).

## 3. Transport: the tests' budgets and EINTR. Accepted by composition
- 16c8ed9: an interrupted timed read or write is retried within the same deadline.
- docs/design/local-owner-transport.md, "The tests' budgets" (d77c2a5): a single measured hang guard, HANG = 120 s, for
  tests, and GREETING_MAX = 2 s on both sides of a greeting (an admission policy).

## 4. The SQL API (crates/api/src/sql.rs, crates/amphora/src/sql.rs): finding 4
- Codex's spec (2026-10-30 23:07Z, seq304), with the Amphora agent's own source 7040b38/32749a0/7780186:
  - Only the success of `Session::validate(sql, false)` marks an entry, and the mark lives on Db's own statement-cache
    entry.
  - A miss or an unmarked hit is validated before prepare or step, and a failure never marks.
  - Internal or reserved private-SQL population never marks.
  - Eviction, replacement, a schema change and a restore drop the mark along with the entry.
  - Owner-local remote Columns and Query go through the same gate.
  - Database execute/migrate and the Session batch keep their own validators.
  - There is no wire or trust flag, no second or unbounded set of texts, and no policy epoch (no dynamic policy
    exists).
- On 2026-10-30 at 23:20Z Codex added: "public validation proven per entry/text, unvalidated internal cache hit cannot
  bypass; remote still server-validates, one-shot validates each time." 7780186 implemented that rule.
- Codex never named a fix commit for step 4. It never saw 7780186's gate message, and on 2026-10-31 at 12:01Z it was
  still asking for one.
- **Owner's rulings (2026-11-06, to amphora-sql-prepared):**
  - (1) 7780186 is not step 4's fix commit. It remains the gated record of the 327a8f9 composition. The fix commit is
    t1/sql-prepared's final head on main 521e4bc, once the seven gates pass there.
  - (2) **Departure from Codex:** a marked hit skips validation on every Session path, one-shot text calls included
    (decision 35 on t1/sql-prepared). The goal's "a repeated text call parses once" stands. Why the departure is safe:
    - `validate(sql, false)` is a pure function of the text: its bound, its parse, the reserved names and the control
      statements;
    - the cache entry's key is that text, and the same validator sets the mark;
    - every safety condition of Codex's spec still holds;
    - `Database::execute`/`migrate` keep `validate_sql` before submit, and that validator never reads a mark;
    - Codex's own W24 trace (2026-10-31 12:01Z) found repeated public validation on kiln's hot text paths to be a real
      cost.
  - The ruling is falsified by a Session path that runs a text no validation accepted while its entry stood (decision
    35's falsifier).

## 5. Owner guards (keepalive) for the Windows host store. Decided 2026-10-30, key amphora-reply-codex-keepalive-decisions-2026-10-30
- Three additive constructors: `Database::open_guarded`, `open_replicated_guarded` and `attach_replicated_guarded`,
  each taking `(…, guards, until)`. `Guards::new(key: [u8;32], keep: Arc<dyn Any+Send+Sync>)` is opaque and
  cloneable. There is no token, no serialization and no wire change.
- Rules:
  1. A guarded open never creates anything: a missing parent gets a typed refusal before OWNERS, the endpoint or the
     lease is touched.
  2. Under the OWNERS lock: the file identity must equal the identity at the path now (never rebind to a new inode),
     then the key must equal the live owner's key, then the owner is reused. The incoming Guards are dropped, and there
     is one group per owner, never accumulated.
  3. A live unguarded owner, or a different key, gets Refusal::Invalid until it really drains. An unguarded open may
     reuse a guarded owner.
  4. A remote predecessor gets a refusal. There is no guarded client and no guarded promotion.
  5. Owner-entry admission comes before the endpoint and the lease are created. Attach probing the lease before its
     bound is a separate bug, with its own test.
  6. OWNERS.lease stays `Weak<File>`. A private `Owned { lease: Arc<File>, guards: Option<Guards> }` is cloned at every
     Arc<File> site (Db, readers, opener, flusher, Shared, the replicated Actor/Machine, replication). The flusher
     captures it after its engine handles.
  7. Portable tests use a Drop witness: no accumulation; the group survives clones, readers and flusher drain; a
     mismatched key, a changed inode, an unguarded predecessor and a remote predecessor each refuse before any effect;
     no directory is created.
- amphora-owner-guards (goal 40a75a8d) builds this, and the owner reviews it against these seven rules.

## 6. Disk stalls and quorum patience (goal 6b112f14)
- Codex required this four times (2026-11-01 18:50Z, 18:51Z; 2026-11-02 01:10Z twice): "bring the production
  disk-stall/quorum decision before code". Codex left replicated.rs and transport unchanged while waiting.
- The test-only measurement harness (c5bd98e, cfg(test) only) was notified under amphora-codex-stalls-harness-2026-11-01.
- **Owner's position:** no production change to replicated.rs for stalls until the decision is written in
  replication.md with its measurements, sources and falsifier, and the owner has agreed to it.

## 7. The replicated application floor (goal d8362e1e, t1/floor-seam 7133e69)
- Codex asked the Amphora agent to own the seam (2026-11-01 18:48Z). Its standing constraints: real member/log-proven
  agreement, never a caller integer or a Ready response; one SDK seam; no second replication controller; and kiln's
  installed floor fails closed in replicated mode until then.
- Codex never answered the 7133e69 design or its questions a-d. The owner answers them when amphora-membership-floor
  asks, using these constraints.

## 8. The replicated class (Durability::Replicated, t1/fast-ack). Ada approved it on 2026-11-01
- This was a heads-up to Codex (amphora-codex-replicated-class-2026-11-01), and Codex never replied.
- The change is additive: Persisted stays the default, and existing opens are unchanged.
- The power-cut harness fix (0b2e3bb, 83f000a) changes tests only.
- Its follow-ups are class-quorum (dcdd872a, db9c7620) and class-metrics (8f0f29ac).

## 9. Other standing rules from Codex
- Records custody (amphora-reply-codex-custody-horizon-2026-10-30): history is per namespace, with a single horizon
  per namespace. Live quotas don't bound history. A second horizon would be a new decision.
- U05 (2026-10-30 23:10Z): don't build a broad async application-submit API without a measured bottleneck.
- Codex never adds a second cache, registry, controller or journal beside an existing owner.

## 10. Composition findings since taking the role (2026-11-06)
- t1/class-quorum's vendor change 0005 (hyper-raft Replica::suspect_held/trust_held) is API that replicated.rs calls.
  Kiln patches hyper-raft to its own copy, so kiln can't take the replicated class's suspicion feed until that copy
  carries 0005, or upstream hyper-raft does. 0003 and 0004 add nothing Amphora calls, which is why kiln builds 4281466
  today. This is a cross-product vendor change, and Ada decides it.
- Owner's decisions on the seam (2026-11-06, to t1/membership-fixes):
  - Refusal gets no Busy or Unknown variants until kiln shows a caller that must branch on them. Refusal is a wire
    enum, changed append-only.
  - A follower's write with no known leader waits to its deadline (Timeout), not NotLeader{None} at once.
  - A ReplicaStatus field appended under postcard needs a transport version bump. serde(default) doesn't make a
    missing trailing field decode, and bootstrapping already carries this flaw.
- Bug to raise with the Amphora agent: unguarded open_replicated/attach_replicated create parent directories through
  canonical_path before later refusals, a side effect before a refusal (found by amphora-owner-guards).
- Ada, 2026-11-06, through the Amphora agent:
  - "Push 521e4bc, repin there." GitHub main is now 521e4bc.
  - For hyper-raft 0005, "Upstream first": amphora-class-quorum prepares it from hyper-raft main, and the push waits
    on Ada.
  - Correction: kiln's newest pin is unify/takeover at 065fdb3, not 4281466. 065fdb3 is NOT on main: it's
    t1/membership-fixes' first commit, unagreed. It has the deadline Ok-while-joint, and lacks transfer and the
    joiner snapshot ceiling. The owner recommends kiln repin at 521e4bc (told under amphora-owner-kiln-pin-065fdb3-20261106).
  - The legacy canonical_path bug is on the Amphora agent's stack.
- 2026-11-06, the kiln repin at 521e4bc (requested through the Amphora agent): stopped and reported
  (amphora-owner-kiln-repin-521e4bc-blocked-20261106). kiln's members.rs (unify/takeover) uses MemberChange and
  ReplicaHandle::change, which exist only at 065fdb3. The owner recommends kiln stay at 065fdb3 until
  t1/membership-fixes is agreed and merged, then repin at that main. Kiln's own patches 0001/0002 have not yet been
  checked against a new base.
- Ada, 2026-11-06, on the kiln pin: "Wait for membership-fixes". Kiln stays at 065fdb3. Once the owner agrees
  t1/membership-fixes and Ada merges it to Amphora main, the owner repins kiln at that main, checking that kiln's
  vendor/patches/amphora 0001 and 0002 re-apply.
- 2026-11-06: Duplicate Systems (kiln agent 01a1b1935e1e7787a5d69076ec3e12e2) is back and again owns kiln's
  unification and vendor tree; the kiln repin is no longer the owner's. DS re-vendors once t1/membership-fixes is
  agreed and merged. DS's kiln patches carry Amphora changes bound for upstream: 0003 (a local read on a replica),
  a leader handing off to a voter (ReplicaHandle::transfer should cover it), and per-learner catch-up in status.
  The Amphora agent turns them into Amphora goals. Each touches replicated.rs, so each comes to the owner for review.
- Proposed merge order (sent to the Amphora agent, key amphora-owner-merge-order-and-metrics-owner-20261106):
  membership-fixes; the fast-ack line (the class, then class-metrics; class-quorum and class-metrics settle their 0005
  numbering); then disk-stalls rebased. election-flake, owner-guards and sql-prepared are independent.
  No Kiln Metrics agent is running (01a14bf7 isn't listed): who agrees metric names is open, and Ada's to assign.
- 2026-11-06: the merge order is recorded in briefs/ROSTER.md; the Amphora agent takes it to Ada as gates pass.
  Patch numbers are settled: class-quorum keeps hyper-raft 0005 and class-metrics takes 0006, both upstream first.
  Correction: Kiln Metrics (01a14bf7) is alive but idle; it has the names and the Amphora agent nudged it; if it
  doesn't answer, it goes to Ada. The owner's earlier "not running" came from a list filtered to working agents.
- 2026-11-06: Kiln Metrics agreed every name:
  - class-metrics' three (amphora-class-metrics/names/v1; flush buckets are powers of two from 62.5 µs to 16.384 s);
  - disk-stalls' two (…/names/v1-disk-stalls);
  - the floor's four amphora_floor_* (amphora-membership-floor/names/v1; events on amphora.floor, e.g. floor.raised).
  Only small closed label sets (quorum, reason). The dashboard rows are kiln-side and need Ada's word to merge.
  The owner checks each branch's code against its agreed names and labels at review.
- 2026-11-06, the Amphora agent: owner-guards' base is t1/sql-prepared's head, or else main 521e4bc; the
  prepared-once fallback is withdrawn (owner-guards is already restacked on 839cd0e, ba13554). store.md's numbers are
  final: sql-prepared 34 and 36, wal-redo 35.
- 2026-11-06, the transport version rule: any branch that changes a message's fields sets wire::VERSION to main's
  VERSION + 1 at its rebase before merge, with a "Versions" line in local-owner-transport.md. Merge order decides the
  number. membership-fixes is 3; class-quorum (Identity.durability) and t1/floor (Request::Application) follow.
- 2026-11-06: correction to the floor's upgrade point 2. The last change's index is not in the marker's point
  (marker() rewrites the applied index on every entry). Replaced by configuration_at (in memory, an overestimate) and
  attestations' `seen`. Binding for the later upstreaming of kiln's patch 0003 (a local read on a replica): refuse a
  local read on a member with no current attestation after J.
- 2026-11-06: the binding that local reads be refused on unattested members after J is WITHDRAWN. It can't protect
  against pre-floor binaries, which don't run the check. The harm is bounded by the floor's promotion guard, kiln
  installing the current binary, and the unattested-members monitor. The local-read decision (t1/replica-local-read
  b40f41d) is agreed, with four additions.
- 2026-11-06: no branch re-vendors hyper-raft on its own. A vendor pin must be a revision upstream holds, and moving
  the shared pin is a coordinated cross-product goal (Amphora, kiln/DS, focal), after Ada's word on the upstream push.
  class-metrics' b4e2a18 is asked off its branch (key amphora-owner-hyper-raft-repin-20261106 to the Amphora agent).
- 2026-11-06, update: d5d5f8f is on hyper-raft's GitHub main (verified with ls-remote), pushed on Ada's word as
  reported, so class-metrics' re-vendor b4e2a18 is legitimate and the revert request is withdrawn. Follow-ups: gates at
  b4e2a18 over the new upstream code; class-quorum's 0005 re-applied on d5d5f8f; DS told the pin moved (key
  amphora-owner-hyper-raft-repin-update-20261106).
- 2026-11-06: Ada capped running agents at four: the seam owner, wal-redo, membership-floor and election-flake. The
  others are stopped, with handoffs. Each stopped branch's open items are in its reply file. The Amphora agent confirms
  it pushed d5d5f8f to hyper-raft main on Ada's word ("Push d5d5f8f to main"), about 18:25Z.
- 2026-11-06: store.md's numbering is final in the branches: 34 and 36 sql-prepared, 35 wal-redo (d83d17f), 37 next
  for wal-redo's delta log. efe0248's move to 36 came from an Amphora agent message that contradicted its own confirmed
  split. If numbers drift again, check the branches, not the messages.
- store.md decision 37 is wal-redo's (the delta log, t1/wal-small-commits step 2). The next store.md decision anyone
  takes is 38. Branches that add store.md decisions are told this at review.
- 2026-11-06: t1/integration-1106 merged to local main at 2472173 (not pushed) on Ada's "Bring it in": membership-fixes,
  interrupt-not-busy, e5-row-value-seek, wal-regression, election-flake, owner-guards and sql-prepared. The owner's
  agreement is FINAL for the four in its files. The replication.rs Machine conflict resolution was checked.
  Open: sql-prepared's bench; owner-guards' Windows tests/database.rs finding. Next for kiln: DS re-vendors at 2472173
  once pushed. main's hyper-raft pin is still 4d72e19.
- 2026-11-06: t1/replicated-line is FINAL at a06e473 (tree 6e5dbc6a), with all seven gates passing. It carries the
  class, class-metrics (hyper-raft d5d5f8f), class-quorum (fixed: VERSION 4, Answered kept), validate_durability before
  the leases, disk-stalls (the falsifier holds on fedora) and the patience metrics. It goes to Ada for main. The
  transport is now VERSION 4, so t1/floor takes 5.
- 2026-11-06: main is a06e473 (the replicated line merged). learner-catch-up merges as t1/integration-1106c 215f166.
  The owner agreed its resolution; main moves after its seven gates.
- 2026-11-07, an incident (reported by amphora-membership-floor, resolved): on adas-mac-studio.local-2 a mutant
  script's loop copied the root filesystem into ~/.cache, about 100 GB over 90 minutes, after an aborted kiln call's
  process ran on. It was stopped by PID with its parent, the copy removed, and the disk is back to normal. Raised for
  Ada through the Amphora agent.
- 2026-11-07, key amphora-owner-change-deadline-flake-ruling-20261107:
  - a_change_past_its_deadline_answers_what_its_stage_knows ("joint configuration not reached in 60000 driven rounds",
    2/553 at loads 48.75 and 59.75) WANTS A GOAL. It tripped inside the gates' band, unlike seed 10 at 125, so some
    wait may be stuck. Capture the pump state at the trip; fix at the cause.
  - J1 (the fork-window reopen) is goal b7e13a35 on t1/fork-window, for owner review. Seed 10 is closed.
- 2026-11-08: the change-deadline flake is root-caused (a test observation gap: joint and way out within one round)
  and fixed test-only in t1/floor e119ff2. Its goal is closed.
- 2026-11-08: t1/floor is FINAL at e119ff2 (tree cd640eca), with all seven gates passing: the replicated application
  floor, goal d8362e1e. It goes to Ada for main; its merge with f99b758 (learner-catch-up) needs the owner's review of
  the resolution. Transport VERSION 5. kiln drops patches 0001 and 0002 after it.
- 2026-11-08: t1/integration-1107 0668f39 (f99b758 + guards-flake + floor): the merge resolution is agreed; main moves
  after its seven gates.
- 2026-11-08: fork-window final head d18ceec (an event rename, agreed). Waiting on the cross-target clippies at d18ceec.
- 2026-11-08: fork-window d18ceec: 6/7 gates are 0; the final waits on Windows clippy (jupiter offline), plus the test-count difference explained by name.
- 2026-11-08: fork-window d18ceec: Windows came via xwin from local-2; held to native-jupiter (the floor precedent); Ada decides any substitute. The count is still open.
- 2026-11-08: integration-1107 fcda916 (+fork-window d18ceec, clean, verified line-identical): final once the seven gates pass (native Windows, count by name).
- 2026-11-08: replica-local-read fed2977 amendments: 2/4 agreed; changes to 1 (unbounded replica log found: no checkpointer on the replicated path), 3 (cap reader, bit), tests and IDs.
- 2026-11-08: fcda916 names are complete; 6/7 gates pass; waiting on native Windows clippy (or Ada on xwin).
- 2026-11-08: FINAL fcda916 (guards-flake, floor, fork-window): seven gates pass. Moving main and the push wait on Ada.
- 2026-11-08: replica-local-read e368c0e agreed for code. Finding: replica log never checkpointed (section 2), sent to the Amphora agent as its own goal (key amphora-owner-finding-replica-log-unbounded-20261108).
- 2026-11-08: main fcda916 is on GitHub (Ada: "Push fcda916 now"). Named for DS: VERSION 5, the metrics reach, and 0003 to be replaced at VERSION 6 (key amphora-owner-main-fcda916-ds-20261108). The replica-log finding is goal 7582f00a; its design comes to me.
- 2026-11-08: replica-local-read 89dba53: L4 red. An open read transaction saw an image installed after its snapshot (engine; reprepare/schema publish suspected). Ruled an engine fix (option A) with instrumentation first.
- 2026-11-08: replica-local-read: the L4 'engine finding' was a test bug (traced); withdrawn, and my snapshot-isolation reading too. Asked for a non-vacuous L4.
- 2026-11-08: replica-local-read db991a0 code agreed; waiting on the seven gates.
- 2026-11-08: FINAL replica-local-read db991a0 (tree 15c60de3), seven gates. The merge waits on Ada.
- 2026-11-08: READERS=4 challenged by DS. Ruled a follow-up toward a shared page pool plus a derived count; db991a0 stays final; Ada decides whether kiln waits.
- 2026-11-08: learner-catch-up's pending macOS clippy is closed: f99b758 is in main fcda916, whose seven gates include aarch64 clippy on local-2 (02:06Z run).
- 2026-11-08: main is db991a0 (local read, READERS=4, on Ada's word); the derived reader bound is a queued goal. Ada: one worker at a time. Owner reviews only when asked; no background watches.
- 2026-11-09: Ada: up to four agents at once, testing batched. One integration branch per batch with the seven gates once on the merged tree (COMMON.md; CLAUDE.md Gates on t1/integration-1109 6d1d2bc). Owner: one design notice before code where asked, one code notice per branch, and the final covers the batch's merged tree after its gates. Running: write-starve, benches, replica-log (7582f00a, design to me), class-and-readers (11209e8d, design to me). Still no watches.
- 2026-11-09: class-and-readers fb18105: blocking, the class entry closes the only way to give a joiner a fate; plus five additions.
- 2026-11-09: replica-log design: agreed in direction, with seven points before code (Persisted's meaning changes; decision 15's cap; the disk budget; coordination with class-and-readers).
- 2026-11-09: class-and-readers 4cfb126: agreed, with a monotone-record refinement to the class supersede.
- 2026-11-09: write-starve 044c44f: G agreed for Ada's word; the merge carries G alone (section 13); four additions before code.
- 2026-11-09: replica-log revision agreed; code notice pending (leader leg, disk budget goal).
- 2026-11-09: class-and-readers 552c0a1: finding: the held gate opens on compatibility, so a joiner can carry two fates. Fix: stated∩conf ⊆ record.
- 2026-11-09: replica-log 92c5a9e: finding: the 2x restart path copies up to 1 GiB on the owner thread. Hold reads, not passes. The rebase onto the Account is theirs.
- 2026-11-09, key amphora-owner-t1-replica-log-dce4aaa-20261109: dce4aaa agreed (hold reads, not passes; a restart only at <= bound/64; busy passes uncounted). Rebase onto class-and-readers' Account now; I review the range-diff only.
- 2026-11-09, key amphora-owner-t1-replica-log-0e8a610-20261109: rebase 0e8a610 (tree 6d96c8fa) onto class-and-readers agreed; handed to the batch.
- 2026-11-09, key amphora-owner-disk-budget-goal-20261109: asked the Amphora agent to create the process disk budget goal and cite it in replication.md.
- 2026-11-09, key amphora-owner-t1-replica-log-e5a6444-20261109: e5a6444 agreed (C6 is the joiner's end-to-end test). Heartbeat Position answers are durable by construction (persisted messages); the harness's hold is of answers in transit, to be noted in its doc.
- 2026-11-11: amphora-seam-owner-2 takes the owner's role (Ada: "Fresh seam-owner agent"), on the same terms; the previous owner's rulings above stand unless a reply says where it departs. Ada's new rule (2026-11-11): no arbitrary constants, learned and stat-derived values only; designs are held to it. Waiting to be asked: the final on t1/integration-1109 once its gates pass, t1/log-bound's design (7582f00a), G's design (9c63f7d4).
- 2026-11-11, key amphora-owner-t1-g-slot-design-20261111: G-slot design: Q1 (hold others' forwards) agreed as necessary (on_base), covering every leader append; Q2 (commit) agreed, with the holder's ReadIndex released only at commit >= the slot-start last index; Q3 NOT agreed: election_tick x TICK is the prototype's 200 ms; learn the retry gap as patience.rs does (cold start the election timeout, ceiling CEILING).
- 2026-11-11, key amphora-owner-t1-log-bound-design-20261111: log-bound design: direction, B, D, Unavailable and N agreed; A changed (count the commits restarts held, against 1-q net of the measured base tail, AC-T1.4.3's 1 ms; no PASTA); C: N per filesystem and live, the raft log and installs named, H as the disk-budget goal's share, a follower past H fences (stated limit, test).
- 2026-11-11, key amphora-owner-t1-log-bound-rev1-20261111: log-bound revision 1 agreed for code, plus: past H a restart is due regardless of the share; AC-T1.4.3's 1 ms is the bench's; concurrent installs a stated limit; SNAPSHOT_BYTES to amphora-constants; 1000 computed from q.
- 2026-11-11, key amphora-owner-t1-log-bound-4a274ec-20261111: log-bound 4a274ec: bytes room and Windows serial agreed; changes: held ends only at an idle step (never under steady load, so the bound degenerates to the disk share; end it at a drained backlog, red test), a leader past room still applies forwards (state the limit); reopen time at a large log to the heavy runs.
- 2026-11-11, keys amphora-owner-t1-constants-code/-design-20261111: the class bound, PEERS and CHANGE_BYTES agreed (no class past MEMBERS: raft.rs:3633); J1 pauses agreed but cap at max(window, waited); MEMBERS and OWNERS_MAX picked. Not agreed: a runtime-learned QUORUM_CEILING (circular; re-derive on more hosts), SNAPSHOT_BYTES from the DB size (it is a memory bound). FLOOR_SESSION from election timeout + patience; one descriptor budget for OWNERS/CONNECTION/READERS as its own goal.
- 2026-11-11, keys amphora-owner-t1-constants-ff65b87/-floor-session-20261111: f7e9c94 and ff65b87 agreed; FLOOR_SESSION from the core agreed in direction, as 2 x quorum_due (the check runs on a fixed counter), stated per role (a follower's own election bound), with a count of sessions ended while still leading.
- 2026-11-11, key amphora-owner-t1-log-bound-004f682-20261111: 004f682 agreed except that the queue readings differ in basis (a step's reading includes its in-hand envelope, the restart's doesn't), so a steady backlog never ends the hold; fix the basis plus an owner-level red test, then the batch.
- 2026-11-11, key amphora-owner-t1-constants-ac1fe63-20261111: ac1fe63 (own_session_lease by role) agreed; the event needs Kiln Metrics' word (replica.own_session_expired, per the events/v1 precedent); a counter or the event named as the record.
- 2026-11-11, key amphora-owner-t1-constants-c892b99-20261111: c892b99 (event renamed, the event is the record) agreed; KM's word before the batch.
- 2026-11-11, key amphora-owner-t1-log-bound-90840aa-20261111: log-bound 90840aa agreed, to the batch; final on the merged tree after the gates.
- 2026-11-11, key amphora-owner-t1-g-slot-878285a-20261111: g-slot 878285a agreed, with departure 1 (slot at the member's next barrier) agreed; departure 2 not: a narrow leader tail rule for the turn after a slot (leader refused up to 8 in a row in W2), the general rule a separate finding.
- 2026-11-12, key amphora-owner-t1-constants-721acf2-20261112: 721acf2 (own-sessions counter, KM-agreed) agreed; nits on the help text and a test of the counter.
- 2026-11-12, key amphora-owner-t1-log-bound-3f7e76a-20261112: log-bound 3f7e76a (gauge rename, KM-agreed) carries the agreement; it is the batch head.
- 2026-11-12, key amphora-owner-t1-constants-e644cba-20261112: constants e644cba agreed; my files on t1/constants agreed through e644cba; final on the batch tree.
- 2026-11-12, key amphora-owner-t1-g-slot-fc28e2d-20261112: g-slot fc28e2d: the turn rule and the harness clock (counted) agreed; nothing held stepped at a non-leader agreed; NOT the receipt held a slot (it changes receive's contract and can starve the quorum check): answer Ok at hold, discard at lost lead with a counted event.
- 2026-11-12, key amphora-owner-t1-g-slot-0030f95-20261112: g-slot 0030f95 agreed (receipt Ok at hold, discard counted at a lost lead); KM's word on two event names before the batch; to the batch.
- 2026-11-12, key amphora-owner-t1-log-bound-7f5447b-20261112: log-bound 7f5447b (test clock, jupiter) agreed, test-only. The Amphora agent is now 01a1d28ffdf37050af219798833ebde6 (the old id is 404).
- 2026-11-12, key amphora-owner-t1-g-slot-48347cd-20261112: g-slot 48347cd (KM agreed the events, doc only) is the batch head; agreed.
- 2026-11-12, key amphora-owner-t1-g-slot-c6ad588-20261112: g-slot c6ad588 (W1's control built, not timed) agreed, test-only.
- 2026-11-12, keys amphora-owner-t1-log-bound-86c78f8-20261112 and amphora-owner-t1-g-slot-w1-stale-20261112: log-bound keep_log(room) agreed (decision 31 cuts to nothing); g-slot W1 fails 3/10 with 'Database snapshot is stale' at c6ad588 alone; root cause owed before the final.
- 2026-11-12: possible production defect for W1's root cause: a replica apply meeting BusySnapshot isn't retried (log-bound's mechanism); pointed g-slot at it.
- 2026-11-12, key amphora-owner-t1-log-bound-2f337a0-20261112: log-bound 2f337a0 (citation, doc only) agreed, the batch head.
