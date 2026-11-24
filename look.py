import json,sys,re,collections,shutil,os
O,R=sys.argv[1],sys.argv[2]
rs=json.load(open(O+"/rank3/rank.json"))
def sub(r):
    c=r["cause"]; sql=r["stmts"]+"\n"+r["queries"]; q=r["queries"]; f=r["features"]
    if c.startswith("PANIC internal error: entered unreachable code: lag") or c.startswith("PANIC internal error: entered unreachable code: nth"): return "CO"
    if c.startswith("PANIC cursor id"): return "DC"
    if c=="TRIGGER": 
        if "natural" in f or "using" in f: return "CZ"
        if "except" in f or "union" in f: return "CY"
        return "CV" if "STRICT" in sql and re.search(r"BEFORE (INSERT|UPDATE)",sql) and "CX" not in sql and not re.search(r"BETWEEN|IS new|= new|NOCASE|RTRIM",sql.split("CREATE TRIGGER")[1].split("END")[0]) else "CX"
    if c!="LOOK": return c
    if "upsert" in f and "strict" in f: return "DA"
    if "upsert" in f and "9223372036854775807" in sql: return "generator (random rowid)"
    if "(" in q and re.search(r"OVER \(",q) and "GROUP BY" in q: return "RW"
    d=r["diff"]
    if "jsonb" in sql and re.search(r"asinh|acosh|atanh|atan|sin|cos|tan|exp|pow|sqrt|ln|log|radians|degrees| / |1e-308",sql) and not "lower(jsonb" in sql: return "CK (JSONB)"
    if re.search(r"octet_length|\blength\(|instr|concat_ws|unicode|substr",sql) and re.search(r"asinh|acosh|atanh|atan|sin|cos|tan|exp|pow|sqrt|ln|log|radians|degrees| / |1e-308",sql): return "CK (inside)"
    if "lower(jsonb" in sql or "upper(jsonb" in sql or "efbfbd" in d: return "ill-formed text"
    if any(x in f for x in ("using","natural")) and re.search(r"FULL JOIN|RIGHT JOIN",q): return "CZ"
    if "recursive" in f: return "CU"
    if any(x in f for x in ("union","except","intersect")) : return "CY"
    if "row value" in f: return "DB"
    return "LOOK"
by=collections.defaultdict(list)
for r in rs:
    r["sub"]=sub(r); by[r["sub"]].append(r)
for k,v in sorted(by.items(),key=lambda x:-len(x[1])):
    o=collections.Counter(r["oracle"] for r in v); st=collections.Counter(",".join(a for a,x in r["settings"].items() if "DIFFERS" in x) or f"rc {r['rc']}" for r in v); ft=collections.Counter(x for r in v for x in r["features"])
    print(f"{k}: {len(v)} {dict(o)} {dict(st)} {dict(ft.most_common(5))}")
for k in ("LOOK","DB","CZ","DA"):
    for r in sorted(by[k],key=lambda r:len(r["stmts"])+len(r["queries"]))[:7 if k=="LOOK" else 2]:
        print("==",k,r["id"],r["features"]); print("   "+"\n   ".join(x[:230] for x in r["stmts"].splitlines()[-5:])); print("   Q:",r["queries"][:380]); print("   "+" | ".join(x.strip() for x in r["diff"].splitlines()[1:9])[:260])
# the record's data: findings.tsv with the sub cause; the two smallest of each cause
shutil.rmtree(R+"/findings",ignore_errors=True); os.makedirs(R+"/findings")
for k,v in by.items():
    v.sort(key=lambda r:len(r["stmts"])+len(r["queries"]))
    for r in v[:2]:
        shutil.copy(r["file"],f"{R}/findings/{re.sub(r'[^A-Za-z0-9]+','-',k).strip('-')}-{r['id']}.md")
with open(R+"/findings.tsv","w") as o:
    for r in sorted(rs,key=lambda r:(r["sub"],len(r["stmts"])+len(r["queries"]))):
        if r["sub"]=="CP" and r["id"]!=by["CP"][0]["id"]: continue
        o.write("\t".join([r["sub"],r["id"],r["kind"],",".join(r["features"]),",".join(k for k,v in r["settings"].items() if "DIFFERS" in v) or f"rc {r['rc']}"," ".join((r["queries"] or r["stmts"]).split())[:240]])+"\n")
json.dump({k:len(v) for k,v in by.items()},open(R+"/causes.json","w"),indent=1)
