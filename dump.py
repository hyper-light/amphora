import sys
tree = sys.argv[1].rstrip("/") + "/"
p = tree + "crates/amphora/src/database/replicated.rs"
s = open(p).read()
old = """        reach(driven, pump, "the group calm", |driven| {
            (0..owners).all(|owner| {
"""
new = """        let t28_from = std::time::Instant::now();
        let t28_rounds = std::cell::Cell::new(0usize);
        reach(driven, pump, "the group calm", |driven| {
            t28_rounds.set(t28_rounds.get() + 1);
            if t28_rounds.get() == ROUNDS {
                for (owner, _) in driven.owners.iter().flatten() {
                    let log = owner.replica.core().raft.log();
                    eprintln!("T28 calm: id {} term {} leader {} leads {} applied {} committed {} last {:?} in_flight {} pending_conf {} conf {:?} flush {} frames {:?} closing_reads {} pending {} sessions_out {} took {:?}", owner.replica.id(), owner.replica.term(), owner.replica.leader(), owner.replica.is_leader(), owner.replica.applied().index, log.committed(), log.last_index(), owner.replica.in_flight(), owner.replica.core().raft.has_pending_conf(), owner.replica.configuration(), matches!(owner.flushing, Flushing::Thread { out: true, .. }), owner.replica.machine().log_frames(), owner.replica.machine().closing_reads(), owner.pending.is_some(), owner.sessions.out, t28_from.elapsed());
                    if owner.replica.is_leader() { eprintln!("T28 tracker {:?}", owner.replica.core().raft.tracker()); }
                }
                eprintln!("T28 moving {} moved {} looked {}", driven.moving(), driven.moved.seen(), driven.looked);
            }
            (0..owners).all(|owner| {
"""
assert s.count(old) == 1, s.count(old)
s = s.replace(old, new)
open(p, "w").write(s)
