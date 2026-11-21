# The audit log (CP-T1.7, AC-T1.7.4; AL.1)

## Decision (lane audit-log): the audit log is RFC 6962's Merkle tree over Records, verified on every read

**What.** `crates/amphora/src/audit.rs` keeps an append-only log of events in one family of any
implementation of Amphora's Records API, through `get` and `commit` alone. Its rows: the head (a
format byte, the size, the root), each event, and each perfect subtree's hash, written once by the
append that completes it (`audit/tree.rs`, a pure core). Opening the log checks the head's root
against the subtrees it names and that no event lies past its end; given a checkpoint a reader saw
before, it checks RFC 6962's consistency proof from it to the head. Reading an event checks its leaf
hash and its inclusion proof against the head. An append is one transaction conditioned on the head
row's revision, so two writers never fork the log.

**Why.** CP-T1.7 asks an audit log that shows tampering, every flipped byte detected on read
(AC-T1.7.4). Crosby and Wallach (USENIX Security 2009) show a hash chain makes an auditor examine
every event between snapshots, while a Merkle tree proves any event in, and any version extending
any earlier one, in a logarithmic number of hashes; RFC 6962 (Certificate Transparency, section 2.1)
fixes that tree's hashing (SHA-256, 0x00 before leaves, 0x01 before nodes) and its proofs, and RFC
9162 sections 2.1.3.2 and 2.1.4.2 their verification. Source notes: `docs/research/audit-log.md`.

**Evidence.** `crates/amphora/tests/audit.rs`: every byte of a 1,000-event log's rows (values and
keys) flipped one at a time, each refused as tampering by the open or the read covering the row
(T-AL.1.1). The core reproduces Certificate Transparency's reference roots, subtree hashes and
proofs (transparency-dev/merkle 75a6539, testonly/constants.go; T-AL.1.2), and every proof of every
tree up to 70 leaves verifies against an independent oracle while every altered one fails (T-AL.1.3).
The cost comparison is `crates/amphora/examples/audit_cost.rs` (the hash chain lives only there, as
the loser of the design); its runs belong in `bench/records`.

**Limits.** Without a checkpoint held outside the store, someone who rewrites every row consistently
(events, hashes and head) is not detected; `al1_a_checkpoint_catches_history_rewritten_whole` shows
both sides. Where kiln keeps checkpoints (signed heads, witnesses) is open.

**Falsified if** a flipped byte of the log's rows reads back as a whole log, or the audit_cost runs
show the Merkle tree's per-event proof cost growing linearly with the log rather than with its log.
