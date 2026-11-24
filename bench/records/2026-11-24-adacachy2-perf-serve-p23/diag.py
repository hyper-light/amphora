#!/usr/bin/env python3
"""P23 diag patch (never for main): per-stage timestamps at the write's path, in either arm.
   diag.py TREE H|M"""
import re, sys
tree, arm = sys.argv[1], sys.argv[2]
def rd(p): return open(f"{tree}/{p}").read()
def wr(p, s): open(f"{tree}/{p}", "w").write(s)
R = "crates/amphora/src/database/replicated.rs"
D = "crates/amphora/src/db.rs"
s = rd(R)
def sub(old, new, count=1, which=0):
    global s
    n = s.count(old)
    if n < 1: raise SystemExit(f"anchor missing: {old[:80]!r}")
    if count == 1 and n != 1: raise SystemExit(f"anchor not unique ({n}): {old[:80]!r}")
    s = s.replace(old, new, count)
def first(old, new):
    global s
    if old not in s: raise SystemExit(f"anchor missing: {old[:80]!r}")
    s = s.replace(old, new, 1)
P = "crate::db::p23"
# leader
sub("        if let Some((_, length)) = self.slots.start(self.at, last, commit, own) {\n            self.control.meter.slot_began(length);",
    f"        if let Some((p23_m, length)) = self.slots.start(self.at, last, commit, own) {{\n            {P}(\"slot_start\", p23_m, last);\n            self.control.meter.slot_began(length);")
sub("                    let forward = self.slots.forward(from, self.at, (message, reservation));",
    f"                    {P}(\"fwd_in\", from, 0);\n                    let forward = self.slots.forward(from, self.at, (message, reservation));")
sub("                    let read = if marked {", f"                    {P}(\"ri_in\", from, u64::from(marked));\n                    let read = if marked {{")
sub("        while let Some((from, (message, _reservation))) = self.slots.ready() {",
    f"        while let Some((from, (message, _reservation))) = self.slots.ready() {{\n            {P}(\"ready\", from, message.msg_type as u64);")
sub("        if refused && appended.checked_sub(last) == entries {",
    f"        {P}(\"appended\", from, appended);\n        if refused && appended.checked_sub(last) == entries {{")
# every step: what was received, and the drive's outcome
sub("        let heartbeat = self.replica.core().raft.config().heartbeat_tick;\n        let now = self.now()?;\n        self.pace.took(&message, now, heartbeat);",
    f"        {P}(\"msg\", from, message.msg_type as u64);\n        let heartbeat = self.replica.core().raft.config().heartbeat_tick;\n        let now = self.now()?;\n        self.pace.took(&message, now, heartbeat);")
sub("        self.release(output)?;\n        // Inputs a pause kept are taken at once, one a step.",
    f"""        {{
            let (p23_c, p23_a) = (self.replica.core().raft.log().committed(), self.replica.applied().index);
            thread_local!(static SEEN: std::cell::Cell<(u64, u64)> = const {{ std::cell::Cell::new((0, 0)) }});
            SEEN.with(|seen| {{
                let (was_c, was_a) = seen.get();
                if p23_c != was_c {{ {P}("commit", p23_c, u64::from(self.replica.is_leader())); }}
                if p23_a != was_a {{ {P}("applied", p23_a, u64::from(self.replica.is_leader())); }}
                seen.set((p23_c, p23_a));
            }});
        }}
        self.release(output)?;
        // Inputs a pause kept are taken at once, one a step.""")
sub("        let took = (self.clock)().saturating_duration_since(at);",
    f"        let took = (self.clock)().saturating_duration_since(at);\n        if took.as_micros() > 300 {{ {P}(\"long_step\", u64::try_from(took.as_micros()).unwrap_or(0), entries); }}")
# the pending write's path
first("        self.tail = None;\n", f"        {P}(\"prep_go\", u64::from(self.replica.is_leader()), self.replica.applied().index);\n        self.tail = None;\n")
sub("                self.tail = Some(target);", f"                {P}(\"tail_wait\", self.replica.applied().index, target);\n                self.tail = Some(target);")
sub("                                pending.phase = Phase::Proposed(value);" if arm in ("H", "H2") else "                            pending.phase = Phase::Proposed(value);",
    (f"                                {P}(\"proposed\", u64::from(self.replica.is_leader()), 0);\n                                pending.phase = Phase::Proposed(value);" if arm in ("H", "H2")
     else f"                            {P}(\"proposed\", u64::from(self.replica.is_leader()), 0);\n                            pending.phase = Phase::Proposed(value);"))
first("                    match self.replica.read(context.clone()) {", f"                    {P}(\"barrier\", 0, 0);\n                    match self.replica.read(context.clone()) {{")
sub("        for (context, _) in output.reads {\n            if self.pending.as_ref().is_some_and(|p| p.context == context) {",
    f"        for (context, _) in output.reads {{\n            if self.pending.as_ref().is_some_and(|p| p.context == context) {{\n                {P}(\"read_state\", 0, 0);")
# outgoing records, by kind
sub("            self.pace.sent(&message, now, lost);", f"            {P}(\"out\", message.to, message.msg_type as u64);\n            self.pace.sent(&message, now, lost);")
first("                if let Some(response) = pending.response.take() {\n                    answer(response, result);\n                }\n            }\n        }\n        for (context, _) in output.reads {",
      f"                if let Some(response) = pending.response.take() {{\n                    {P}(\"answered_apply\", 0, 0);\n                    answer(response, result);\n                }}\n            }}\n        }}\n        for (context, _) in output.reads {{")
sub("        ) && let Some(response) = pending.response.take()\n        {\n            // The write's own token (`tokens`): its index, and its stamp for its time.",
    f"        ) && let Some(response) = pending.response.take()\n        {{\n            {P}(\"answered_commit\", index, 0);\n            // The write's own token (`tokens`): its index, and its stamp for its time.")
if arm in ("H", "H2"):
    sub("        // A restart due and ready runs before the write lock is lent, as after a session run\n        // inline;",
        f"        {P}(\"hand_begin\", u64::from(self.replica.is_leader()), 0);\n        // A restart due and ready runs before the write lock is lent, as after a session run\n        // inline;")
    sub("        self.control.local.unhold_reads()?;\n        let base = match self.replica.machine().session_base() {",
        f"        self.control.local.unhold_reads()?;\n        {P}(\"hand_flushed\", 0, 0);\n        let base = match self.replica.machine().session_base() {{")
    sub("        self.sessions.hand(Box::new(move |db: &mut Db| {\n            let ran = crate::replication::prepare_at(db, &base, |db| {",
        f"        {P}(\"handing\", 0, 0);\n        self.sessions.hand(Box::new(move |db: &mut Db| {{\n            {P}(\"w_begin\", 0, 0);\n            let ran = crate::replication::prepare_at(db, &base, |db| {{")
    sub("            .map(|(prepared, answer)| (encoded(&prepared), answer));\n            (ran, operation)",
        f"            .map(|(prepared, answer)| (encoded(&prepared), answer));\n            {P}(\"w_end\", 0, 0);\n            (ran, operation)")
    sub("        self.replica.hold_apply(true);\n        pending.phase = Phase::Running;",
        f"        {P}(\"handed\", 0, 0);\n        self.replica.hold_apply(true);\n        pending.phase = Phase::Running;")
    sub("            self.replica.hold_apply(false);\n            self.ran = Some(ran);",
        f"            {P}(\"taken\", 0, 0);\n            self.replica.hold_apply(false);\n            self.ran = Some(ran);")
    sub("        let Some((ran, operation)) = self.ran.take() else {\n            return Ok(());\n        };",
        f"        let Some((ran, operation)) = self.ran.take() else {{\n            return Ok(());\n        }};\n        {P}(\"decide\", 0, 0);")
if arm in ("H2", "M"):
    sub("            let clock = Arc::clone(&self.clock);\n            self.replica.machine_mut().prepare_db(|db| {",
        f"            let clock = Arc::clone(&self.clock);\n            {P}(\"sess_begin\", u64::from(self.replica.is_leader()), 0);\n            self.replica.machine_mut().prepare_db(|db| {{")
    sub("        // The preparation ran the caller's work, which takes time of its own: its deadline is\n        // judged at the clock's reading after it, never at the step's start.\n        self.at = (self.clock)();",
        f"        {P}(\"sess_end\", 0, 0);\n        // The preparation ran the caller's work, which takes time of its own: its deadline is\n        // judged at the clock's reading after it, never at the step's start.\n        self.at = (self.clock)();")
if arm == "H2":
    sub("        } else if matches!(pending.author, Author::Application(_)) {", "        } else if matches!(pending.author, Author::Application(_)) && !crate::db::p23_inline() {")
wr(R, s)
d = rd(D)
d += """
/// P23 diag (never for main): with P23_ALT set, every second application write runs inline on its owner.
pub(crate) fn p23_inline() -> bool {
    static SEEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    std::env::var_os("P23_ALT").is_some() && SEEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 2 == 1
}

/// P23 diag (never for main): a stage's time, its thread's CPU and run-queue wait, on stderr.
pub(crate) fn p23(stage: &str, a: u64, b: u64) {
    use std::io::Write;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let sched = std::fs::read_to_string("/proc/thread-self/schedstat").unwrap_or_default();
    let line = format!("P23 {now} {stage} {a} {b} {}\\n", sched.trim());
    let _ = std::io::stderr().write_all(line.as_bytes());
}
"""
old = """        self.exec("BEGIN IMMEDIATE")?;
        let pager = self.conn.get_pager();
        let result = self.page_one().and_then(|page_one| {
            pager.record_access(bound).map_err(fail)?;
            let answer = f(self)?;
            let access = pager.take_access().map_err(fail)?;
            let pages = pager.transaction_image(max_bytes).map_err(fail)?;
            let reads = (!access.overflowed()).then(|| access.read().collect());
            Ok((answer, pages, Traced { reads, page_one }))
        });
        pager.end_access();
        // A prepared transaction never leaves an engine commit marker, even when capture fails.
        self.exec("ROLLBACK")?;
        result"""
new = """        p23("s_call", 0, 0);
        self.exec("BEGIN IMMEDIATE")?;
        p23("s_begun", 0, 0);
        let pager = self.conn.get_pager();
        let result = self.page_one().and_then(|page_one| {
            pager.record_access(bound).map_err(fail)?;
            let answer = f(self)?;
            p23("s_ran", 0, 0);
            let access = pager.take_access().map_err(fail)?;
            let pages = pager.transaction_image(max_bytes).map_err(fail)?;
            p23("s_imaged", u64::try_from(pages.len()).unwrap_or(0), 0);
            let reads = (!access.overflowed()).then(|| access.read().collect());
            Ok((answer, pages, Traced { reads, page_one }))
        });
        pager.end_access();
        // A prepared transaction never leaves an engine commit marker, even when capture fails.
        self.exec("ROLLBACK")?;
        p23("s_rolled", 0, 0);
        result"""
if old not in d: raise SystemExit("prepare_traced anchor missing")
d = d.replace(old, new, 1)
wr(D, d)
V = "vendor/hyper-raft/crates/hyper-durable/src/replica.rs"
L = "vendor/hyper-raft/crates/hyper-durable/src/lib.rs"
if arm == "H2":
    v = rd(V)
    old = "        let issued = self.node.advance_issued(ready);\n        self.must(issued)?;\n        if let Some(hard) = hard {\n            self.issued = hard;\n        }\n        let tip = self.tip()?;"
    if v.count(old) != 1: raise SystemExit("hyper-durable take_ready anchor: %d" % v.count(old))
    v = v.replace(old, "        crate::p23d(\"wstart\", number, u64::from(carries) * 1000 + u64::from(state == State::Empty) * 100000 + u64::try_from(messages.len()).unwrap_or(0));\n" + old, 1)
    old = "        if write.state == State::Submitted {\n            self.last_durable = Some(now);"
    if v.count(old) != 1: raise SystemExit("hyper-durable durable anchor: %d" % v.count(old))
    v = v.replace(old, "        if let Kind::Ready(n) = &write.kind { crate::p23d(\"wdone\", *n, now.saturating_sub(write.submitted) / 1000); }\n" + old, 1)
    wr(V, v)
    l = rd(L)
    l += """
/// P23 diag (never for main): a write's start and end, on stderr.
pub fn p23d(stage: &str, a: u64, b: u64) {
    use std::io::Write;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let sched = std::fs::read_to_string("/proc/thread-self/schedstat").unwrap_or_default();
    let line = format!("P23 {now} {stage} {a} {b} {}\\n", sched.trim());
    let _ = std::io::stderr().write_all(line.as_bytes());
}
"""
    wr(L, l)
print("patched", tree, arm)
