# A forwarded proposal that meets an election

Source notes for replication.md, "A forward that meets an election".

## etcd-io/raft at c51f010c851edd0dada5af41bafbf68e54617767 (2026-10-23)

- raft.go lines 86-88: `ErrProposalDropped is returned when the proposal is ignored by some cases,
  so that the proposer can be notified and fail fast.`
- raft.go line 1305, stepLeader, `pb.MsgProp` while `r.leadTransferee != None`: "transfer leadership
  to %x is in progress; dropping proposal". It returns `ErrProposalDropped`.
- raft.go line 1677, stepCandidate, `pb.MsgProp`: "no leader at term %d; dropping proposal". It
  returns `ErrProposalDropped`.
- raft.go line 1714, stepFollower, `pb.MsgProp` with `r.lead == None`: the same refusal. With a
  leader known, it forwards to `r.lead`.
- node.go line 222, `Propose`: a request "can be lost without notice, therefore it is user's job to
  ensure proposal retries."

A forward that reaches a member during a transfer, or between terms, is refused there, and resolving
it is the proposer's job. hyper-raft keeps the same three cases: `Dropped::Transferring` in
`propose`, and `Dropped::NoLeader` in `step_follower` and `step_candidate`. Its `Dropped`
documentation classes both as ordinary refusals, retried at the next leader.

## Ongaro, Consensus: Bridging Theory and Practice (Stanford, 2014), chapter 6

- Section 6.2: a client that contacts a server that doesn't lead is refused and redirected, and it
  retries at another server.
- Section 6.3: a command whose answer is lost must still be applied only once, so a retry needs the
  earlier outcome to be known. Amphora's owner never replays an evaluated session. Its caller hears
  UnknownOutcome.
