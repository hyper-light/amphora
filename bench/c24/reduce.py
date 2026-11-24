import os, re, sqlite3, subprocess, sys
O=os.path.expanduser("~/.cache/amphora-perf/vdbe-c24-out"); EXE=O+"/c24_sql_base"
def w(v): return "NULL" if v is None else ("'%s'"%v if isinstance(v,str) else (repr(v) if isinstance(v,float) else str(v)))
def ans(st,q):
    con=sqlite3.connect(":memory:", isolation_level=None)
    try:
        for s in st: con.execute(s)
        rows=sorted(",".join(w(v) for v in r) for r in con.execute(q).fetchall())
    except sqlite3.Error: return None
    return "%d:%s"%(len(rows),";".join(rows))
def flatten(db):
    st=[]
    for kind,head,rows in db:
        if kind=="i":
            if rows: st.append(head+" VALUES "+", ".join(rows))
        else: st.append(head)
    return st
def bad(db,q,setting):
    st=flatten(db); a=ans(st,q)
    if a is None: return False
    f=O+"/red-%d.tsv"%os.getpid()
    open(f,"w").write("db\tx\tx\n"+"".join("sql\t%s\n"%s for s in st)+"q\t%s\t%s\n"%(q,a))
    try: p=subprocess.run([EXE,"--nocapture"],env=dict(os.environ,C24_SQL=f,C24_ONE=setting),stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True,timeout=10)
    except subprocess.TimeoutExpired: return False
    return re.search(r"== \[[^\]]*\] 1 of 1 differ",p.stdout) is not None and "ERROR" not in p.stdout and "PANIC" not in p.stdout
def load(tsv,needle):
    db=None
    for line in open(tsv):
        f=line.rstrip("\n").split("\t")
        if f[0]=="db": cur=[]
        elif f[0]=="sql": cur.append(f[1])
        elif f[0]=="q" and needle in f[1]: return cur,f[1]
def parse(st):
    out=[]
    for s in st:
        m=re.match(r"(INSERT INTO \w+) VALUES (.*)$",s)
        if m: out.append(["i",m.group(1),re.findall(r"\([^()]*\)",m.group(2))])
        else: out.append(["s",s,[]])
    return out
st,q=load(sys.argv[1],sys.argv[2]); setting=sys.argv[3]
db=parse(st); n=0
assert bad(db,q,setting),"not bad to begin with"
changed=True
while changed:
    changed=False
    for i in range(len(db)-1,-1,-1):
        t=db[:i]+db[i+1:]
        if bad(t,q,setting): db=t; changed=True; continue
        if db[i][0]=="i":
            size=max(1,len(db[i][2])//2)
            while size>=1:
                j=0
                while j<len(db[i][2]):
                    rows=db[i][2][:j]+db[i][2][j+size:]
                    t=db[:i]+[["i",db[i][1],rows]]+db[i+1:]
                    if bad(t,q,setting): db=t; changed=True
                    else: j+=size
                size//=2
for s in flatten(db): print(s)
print(q); print("SQLite",ans(flatten(db),q))
