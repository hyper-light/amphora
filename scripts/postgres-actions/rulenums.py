"""Each rule number's left side, length, gram.y line, action hash and first line.

python3 scripts/postgres-actions/rulenums.py <repo> <rule>...
"""
import re, sys
root = sys.argv[1]
src = open(root + "/crates/pg-parser/src/gram_tables.rs").read()
def arr(name):
    return re.search(r"pub static " + name + r": \[[^;]+; \d+\] = \[(.*?)\];", src).group(1)
r1 = [int(x) for x in arr("R1").split(", ")]
r2 = [int(x) for x in arr("R2").split(", ")]
rline = [int(x) for x in arr("RLINE").split(", ")]
hashes = arr("ACTION_HASH").split(", ")
tname = re.findall(r'"((?:[^"\\]|\\.)*)"', arr("TNAME"))
gram = open(root + "/vendor/postgres/src/backend/parser/gram.y").read().split("\n")
for a in sys.argv[2:]:
    r = int(a)
    print(r, tname[r1[r]], "len", r2[r], "line", rline[r], hashes[r], "|", gram[rline[r]-1].strip()[:100])
