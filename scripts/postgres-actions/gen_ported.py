"""ported()'s generated arms in crates/pg-parser/src/gram.rs from ported.rules ("<rule> <action>" a line).

python3 scripts/postgres-actions/gen_ported.py <repo> scripts/postgres-actions/ported.rules
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
for line in open(sys.argv[2]).read().strip().split("\n"):
    rule, action = line.split(" ", 1)
    r = int(rule)
    h = re.fullmatch(r"Some\(0x([0-9a-f]{16})\)", hashes[r]).group(1)
    text = gram[rline[r] - 1]
    text = re.sub(r"\{.*\}?", "", text)
    text = re.sub(r"%prec \w+", "", text)
    text = text.split(":", 1)[1] if re.match(r"^\w+:", text) else text
    text = " ".join(text.replace("|", " ", 1).split()) if text.strip().startswith("|") else " ".join(text.split())
    if r2[r] == 0:
        text = "/* EMPTY */"
    hx = "_".join(h[i:i + 4] for i in range(0, 16, 4))
    print(f"        // {tname[r1[r]]}: {text}\n        {r} => (0x{hx}, {action}),")
