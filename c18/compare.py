import re,sys
def read(path):
    out={}; sql=None
    for line in open(path):
        line=line.rstrip("\n")
        if line.startswith("--- "):
            sql=re.sub(r"^--- (\[[^\]]*\] )?","",line)
            n=0
            while (sql,n) in out: n+=1
            key=(sql,n); out[key]=None
        elif sql and out[key] is None:
            out[key]=line
    return out
a=read(sys.argv[1]); s=read(sys.argv[2])
bad=0
for k,want in s.items():
    got=a.get(k)
    if got is not None and got.startswith("ERROR "): got=got.replace("ERROR Parse error: ","ERROR ")
    if got!=want:
        bad+=1; print(f"{k[0]}\n    SQLite  {want}\n    Amphora {got}")
print(f"{bad} of {len(s)} differ")
