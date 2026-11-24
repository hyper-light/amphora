"""Lane C26: a probe's log by database: its name without numbers, how many questions differ, and the first that does."""
import re, sys
text = open(sys.argv[1], errors="replace").read()
most = int(sys.argv[2]) if len(sys.argv) > 2 else 12
tail = [l for l in text.split("\n") if l.startswith("ASKED")]
blocks = re.split(r"^##### ", text, flags=re.M)[1:]
groups = {}
for block in blocks:
    name, _, body = block.partition("\n")
    body = body.split("ASKED ")[0]
    serial = body.split("  ---- parallel")[0]
    entries = re.findall(r"^  (DIFFER|WORDS|FILL) (.*?)(?=^  (?:DIFFER|WORDS|FILL) |\Z)", serial, flags=re.M | re.S)
    kind = re.sub(r"^drawn.*", "drawn", re.sub(r"\(parallel.*", "", name)).strip()
    key = kind.split(":")[0]
    groups.setdefault(key, []).append((name, entries))
print("\n".join(l[:300] for l in tail))
for key, found in sorted(groups.items(), key=lambda kv: -len(kv[1])):
    total = sum(len(e) for _, e in found)
    print(f"== {key}: {len(found)} databases, {total} lines")
    shown = 0
    for name, entries in found:
        if shown >= (most if key == "drawn" else 2): break
        shown += 1
        print(f"  -- {name}")
        for kind, body in entries[: (1 if key == "drawn" else 2)]:
            print("     " + kind + " " + body.strip().replace("\n", "\n     "))
