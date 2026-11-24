import subprocess, sys, re, os
root = os.path.expanduser("~/.cache/amphora-perf/postgres-remote")
os.chdir(root)
A = "crates/pg-analysis/src/"
M = [
 ("M1: a LATERAL item sees none of its left ones", A+"expr.rs", "if item.lateral_only && !scope.lateral {\n                    continue;", "if item.lateral_only {\n                    continue;"),
 ("M2: array_position's NULL finds nothing", A+"func.rs", "    let sought = sought.ok_or_else(mismatch)?;\n", "    let sought = sought.ok_or_else(mismatch)?;\n    if sought.is_none() {\n        return Ok(None);\n    }\n"),
 ("M3: a NULL comes first", A+"lower.rs", '" ASC NULLS LAST"', '" ASC"'),
 ("M4: an ungrouped column isn't refused", A+"stmt.rs", "if self.aggregates || !select.group.is_empty() {", "if false {"),
 ("M5: no table's primary key groups its columns", A+"stmt.rs", "!key.is_empty() && key.iter().all(is_grouped)", "key.is_empty() && key.iter().all(is_grouped) && table > 99"),
 ("M6: array_out quotes no NULL", A+"array.rs", '                    || value.eq_ignore_ascii_case("NULL")\n', ""),
 ("M7: a subquery's column is of no table", A+"lower.rs", "Some(output) => id = *output,", "Some(_) => return Ok((0, 0)),"),
 ("M8: pg_inherits isn't served", "crates/pg-catalog/src/lib.rs", "pub static CATALOGS: [&Catalog; 6] = [\n    &data::PG_NAMESPACE,\n    &data::PG_TYPE,\n    &data::PG_CLASS,\n    &data::PG_ATTRIBUTE,\n    &data::PG_PARTITIONED_TABLE,\n    &data::PG_INHERITS,\n];", "pub static CATALOGS: [&Catalog; 5] = [\n    &data::PG_NAMESPACE,\n    &data::PG_TYPE,\n    &data::PG_CLASS,\n    &data::PG_ATTRIBUTE,\n    &data::PG_PARTITIONED_TABLE,\n];"),
 ("M9: current_schemas leaves pg_catalog out", A+"func.rs", ".fetch(implicit)", ".fetch(false)"),
 ("M10: a negative LIMIT isn't refused", A+"lower.rs", "if value < 0 {", "if value < i64::MIN + 1 {"),
]
only = sys.argv[1:]
def test():
    r = subprocess.run(["nice","-n","10","cargo","test","-p","amphora","--locked","--test","postgres_protocol","t_pg_12_1"],capture_output=True,text=True)
    out = r.stdout + r.stderr
    steps = re.search(r"(\d+) steps", out)
    err = re.search(r"^error(?!: test failed).*", out, re.M)
    return r.returncode, (steps.group(0) + " wrong") if steps else (err.group(0) if err else "no steps wrong")
def apply(path, old, new):
    s = open(path).read()
    if s.count(old) < 1: return False
    open(path, "w").write(s.replace(old, new)); return True
for name, path, old, new in M:
    if only and name.split(":")[0] not in only: continue
    if not apply(path, old, new):
        print(name, "- NOT APPLIED", flush=True); continue
    code, what = test()
    print(f"{name}: exit {code}, {what}", flush=True)
    subprocess.run(["git","checkout","-q","-f","--",path])
