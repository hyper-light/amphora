# usage: plainrun.sh BENCH LIB DIR: each plain file replayed; the queries that differ, the settings they differ at, both answers
for f in "$3"/*.md; do echo "##### $(basename "$f" .md)"; timeout 30 "$1" --oracle --sqlite "$2" --replay "$f" > "$3/out.txt" 2>&1; rc=$?
python3 -I - "$3/out.txt" $rc <<'PY'
import re,sys,collections
t=open(sys.argv[1]).read(); rc=sys.argv[2]
cur=None; d=collections.OrderedDict(); same=0; q=None
for b in re.split(r"\n(?=#### |== )", "\n"+t):
    b=b.strip("\n")
    m=re.match(r"#### (.*?): (.*)", b)
    if m:
        cur=m.group(1)
        if "refused by Amphora [R" in b: print("   refused:", re.sub(r".*refused by Amphora ","",b.splitlines()[0])[:330]) if cur=="default" else None
        continue
    m=re.match(r"== (same|DIFFERS) (.*)", b)
    if m:
        if m.group(1)=="same": same+=1; continue
        s=re.search(r"-- SQLite:\s+(.*?)\n-- Amphora:\s+(.*?)(?=\n-- Amphora's plan|\Z)", b, re.S)
        d.setdefault(m.group(2),[[],s]); d[m.group(2)][0].append(cur)
for k,(st,s) in d.items():
    f=lambda x:" / ".join(y.strip() for y in x.strip().splitlines())[:220]
    print("   DIFFERS",",".join(st),"::",k[:300]); print("      SQLite ",f(s.group(1)) if s else "?"); print("      Amphora",f(s.group(2)) if s else "?")
p=re.findall(r"panicked at (.*?)\n(.*)", t)
print("   rc",rc,"same",same, ("PANIC "+p[0][0][-60:]+" "+p[0][1][:200]) if p else "")
PY
done
