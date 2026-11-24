"""tests/pgbench.rs's PGBENCH entries: stmts.txt's statements by index beside pg-oracle's texts in oracle.out.

python3 scripts/postgres-actions/gen_test.py scripts/postgres-actions 0,1,...,40
"""
import sys
d = sys.argv[1]
stmts = open(d + "/stmts.txt", encoding="utf-8").read().split("\n")[:-1]
raw = open(d + "/oracle.out", "rb").read()
frames, i = [], 0
while i < len(raw):
    nl = raw.index(b"\n", i)
    head = raw[i:nl].decode().split(" ")
    assert head[0] == "T", head
    n = int(head[1])
    frames.append(raw[nl + 1:nl + 1 + n].decode("utf-8"))
    assert raw[nl + 1 + n:nl + 2 + n] == b"\n"
    i = nl + 2 + n
assert len(frames) == len(stmts), (len(frames), len(stmts))
keep = [int(x) for x in sys.argv[2].split(",")]
out = []
for k in keep:
    s, t = stmts[k], frames[k]
    assert '"#' not in s and '"#' not in t
    out.append(f'    (\n        r#"{s}"#,\n        r#"{t}"#,\n    ),')
print("\n".join(out))
