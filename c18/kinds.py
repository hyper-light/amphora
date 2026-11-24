import os,re,sys
t=open(sys.argv[1]).read().split("\n")
seen={}
i=0
while i<len(t):
    m=re.match(r"(reads plain|reads indexed|writes) (default|parallel, statistics|parallel|statistics): (.*)",t[i])
    if m and i+2<len(t):
        key=m.group(3); seen.setdefault(key,[t[i+1].strip(),t[i+2].strip(),[]])[2].append(m.group(1)[:8]+"/"+m.group(2)); i+=3
    else: i+=1
kinds={}
for sql,(s,a,w) in sorted(seen.items(), key=lambda x: len(x[0])):
    k=(re.sub(r"[a-z0-9]+\.[kv]|: [a-z0-9]+$","X",s)[:50] if "error" in s else "rows")+" -> "+(re.sub(r"[a-z0-9]+\.[kv]|: [a-z0-9]+$","X",a)[:60] if "error" in a or "PANIC" in a else "rows")
    kinds.setdefault(k,[]).append((sql,s,a,w))
print(len(seen),"statements differ")
n=int(sys.argv[2]) if len(sys.argv)>2 else 3
for k,v in sorted(kinds.items(), key=lambda x:-len(x[1])):
    print("##",len(v),k)
    for sql,s,a,w in v[:n]:
        print("  ",len(w),sql[:900]); print("      ",s[:200]); print("      ",a[:200])
