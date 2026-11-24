# usage: plainrun27.sh BENCH LIB DIR: each plain file replayed; the queries that differ, the settings they differ at, both answers; SAME ones named
for f in "$3"/*.md; do echo "##### $(basename "$f" .md)"; timeout 60 "$1" --oracle --sqlite "$2" --replay "$f" > "$3/out.txt" 2>"$3/err.txt"; rc=$?
python3 -I - "$3/out.txt" $rc "$3/err.txt" <<'PY'
import re,sys,collections
t=open(sys.argv[1],errors="replace").read(); rc=sys.argv[2]; e=open(sys.argv[3],errors="replace").read()
cur=None; d=collections.OrderedDict(); same=collections.OrderedDict()
for b in re.split(r"\n(?=#### |== )", "\n"+t):
    b=b.strip("\n")
    m=re.match(r"#### (.*?): (.*)", b)
    if m:
        cur=m.group(1)
        if "refused by Amphora [R" in b and cur=="default": print("   refused:", re.sub(r".*refused by Amphora ","",b.splitlines()[0])[:330])
        continue
    m=re.match(r"== (same|DIFFERS) (.*)", b)
    if m:
        s=re.search(r"-- SQLite:\s+(.*?)\n-- Amphora:\s+(.*?)(?=\n-- Amphora's plan|\Z)", b, re.S)
        (same if m.group(1)=="same" else d).setdefault(m.group(2),[[],s])[0].append(cur)
f=lambda x:" / ".join(y.strip() for y in x.strip().splitlines())[:260]
for k,(st,s) in d.items():
    print("   DIFFERS",",".join(st),"::",k[:400]); print("      SQLite ",f(s.group(1)) if s else "?"); print("      Amphora",f(s.group(2)) if s else "?")
for k,(st,s) in same.items():
    if k not in d: print("   same    ::",k[:200],"->",f(s.group(1))[:120] if s else "?")
p=re.findall(r"panicked at (.*?)\n(.*)", e)
print("   rc",rc, ("PANIC "+p[0][0][-60:]+" "+p[0][1][:200]) if p else "")
PY
done
