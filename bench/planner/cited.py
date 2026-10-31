#!/usr/bin/env python3
"""Every baseline number section 9 of docs/research/planner.md ("The changes ranked") cites, as
`cited.tsv` lists them, recomputed from step 3's records (the JSONL passes beside
bench/records/2026-10-26-jupiter-planner-baseline.md) by `summary.py`'s definitions, and compared
with the number as written there: a number differs when the recomputed value, written in the row's
format, is not the text cited, or when a row says the record's own text holds the number and it
doesn't. Prints each number that differs and, last, how many do (the section's test: exactly 0).

A query and engine's record is the one `summary.py --best` takes over the passes a row names (each
query's fastest pass; their estimates and counts are the same). q-error, prefixes, the largest
intermediate result, planning and run times and the geometric means are `summary.py`'s; the few
measures it has no function for are defined where they are computed below.

With --job, a clone of gregrahn/join-order-benchmark at a396036, the rows read from JOB's text are
checked too: the equality terms a query has beyond those that connect each class of columns they
equate, and the connected subsets of its join graph.

usage: python3 -I bench/planner/cited.py [--job DIR]
"""

import importlib.util
import json
import math
import pathlib
import re
import statistics
import sys

HERE = pathlib.Path(__file__).resolve().parent
RECORDS = HERE.parent / "records"
RECORD = RECORDS / "2026-10-26-jupiter-planner-baseline.md"
PREFIX = "2026-10-26-jupiter-planner-"

# summary.py is loaded by path (python3 -I keeps the script's directory off sys.path), leaving no
# bytecode beside it.
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("summary", HERE / "summary.py")
summary = importlib.util.module_from_spec(spec)
spec.loader.exec_module(summary)


def load(passes):
    """Each (bench, query, target)'s record, as `summary.py --best` takes it over `passes`."""
    queries = {}
    for name in passes:
        label = None
        with open(RECORDS / f"{PREFIX}{name}.jsonl") as f:
            for line in f:
                r = json.loads(line)
                if r["record"] == "run":
                    label = r["bench"] + (f" SF {r['sf']:g}" if r.get("sf") else "")
                elif r["record"] == "query":
                    key = (label or r["bench"], r["query"], r["target"])
                    old = queries.get(key)
                    if old is not None and summary.best(old) is not None and (summary.best(r) is None or summary.best(old) <= summary.best(r)):
                        continue
                    queries[key] = r
    return queries


def per_query(queries, bench):
    """`summary.py`'s per-query line: prefixes, max and largest result's q-error, plan and run times."""
    out = {}
    for (b, name, target), r in queries.items():
        if b != bench:
            continue
        line = out.setdefault(name, {"ms": {}, "record": {}})
        line["ms"][target] = summary.best(r)
        line["record"][target] = r
        if target == "amphora":
            pre, _ = summary.prefixes(r)
            tables = {}
            for p in pre:
                tables[p["plan"]] = max(tables.get(p["plan"], 0), p["position"] + 1)
            big = max(pre, key=lambda p: p["act"] * p["calls"], default=None)
            line.update(prefixes=pre, tables=max(tables.values(), default=0), max_q=max((p["q"] for p in pre), default=None),
                        largest_q=big["q"] if big else None, plan_ms=summary.best(r, "prepare_ms"))
    return out


def stat(values, name):
    if name == "count":
        return len(values)
    if name == "max":
        return max(values)
    if name == "min":
        return min(values)
    if name == "median":
        return summary.percentile(values, 50)
    return summary.percentile(values, int(name[1:]))


def chosen(lines, selector):
    """The queries a selector names: all, largest>=N or largest<N (the largest result's q-error),
    tables>N or tables=A-B (the tables a query's plan joins)."""
    if selector == "all":
        return dict(lines)
    m = re.fullmatch(r"(largest|tables)(>=|<|>|=)([\d.]+)(?:-(\d+))?", selector)
    field, op, a, b = m.group(1), m.group(2), float(m.group(3)), m.group(4)
    key = "largest_q" if field == "largest" else "tables"
    test = {">=": lambda v: v >= a, "<": lambda v: v < a, ">": lambda v: v > a, "=": lambda v: a <= v <= float(b)}[op]
    return {n: l for n, l in lines.items() if l.get(key) is not None and test(l[key])}


def geomean_over_amphora(lines, engine):
    """`summary.py`'s comparison: the engine's time over Amphora's, geometric mean over the queries both
    finished; with how many there are and on how many the engine was the faster."""
    pairs = [(l["ms"]["amphora"], l["ms"][engine]) for l in lines.values() if l["ms"].get("amphora") and l["ms"].get(engine)]
    return math.exp(sum(math.log(b / a) for a, b in pairs) / len(pairs)), len(pairs), sum(1 for a, b in pairs if b < a)


def inner_scans(lines):
    """Plans whose loop after the first scans a table whole (`"type": "scan"` from a table in the
    query's EXPLAIN QUERY PLAN FORMAT=JSON, its top level) for each row before it: a nested loop with
    no index on its inner side. Each with the prefix before it, estimated and counted."""
    out = []
    for name, l in lines.items():
        r = l["record"].get("amphora")
        loops = [n for n in (r.get("plan") or {}).get("nodes", []) if n.get("parent") is None and n["op"].get("type") in ("scan", "search")]
        for position, node in enumerate(loops):
            if position > 0 and node["op"].get("type") == "scan" and node["op"].get("source") == "table":
                outer = loops[position - 1]["op"]
                alias = outer.get("alias") or outer.get("table")
                out.append((name, next(p for p in l["prefixes"] if p["table"] == alias and p["position"] == position - 1)))
    return out


def ranks(values):
    order = sorted(range(len(values)), key=lambda i: values[i])
    out = [0.0] * len(values)
    i = 0
    while i < len(order):
        j = i
        while j + 1 < len(order) and values[order[j + 1]] == values[order[i]]:
            j += 1
        for k in range(i, j + 1):
            out[order[k]] = (i + j) / 2 + 1
        i = j + 1
    return out


def spearman(lines):
    """The rank correlation (Spearman's, ties averaged) of each query's estimated cost, the
    `total_cost` of its plan's last loop in EXPLAIN QUERY PLAN FORMAT=JSON, with Amphora's time."""
    costs, times = [], []
    for l in lines.values():
        loops = [n for n in l["record"]["amphora"]["plan"]["nodes"] if n.get("parent") is None and n["op"].get("estimate")]
        costs.append(loops[-1]["op"]["estimate"]["total_cost"])
        times.append(l["ms"]["amphora"])
    x, y = ranks(costs), ranks(times)
    mx, my = statistics.mean(x), statistics.mean(y)
    return sum((a - mx) * (b - my) for a, b in zip(x, y)) / math.sqrt(sum((a - mx) ** 2 for a in x) * sum((b - my) ** 2 for b in y))


def compute(kind, args, lines):
    if kind == "qerr":  # bench joins|all stat: q-error over the prefixes with that many joins
        joins, name = args
        qs = [p["q"] for l in lines.values() for p in l.get("prefixes", []) if joins == "all" or p["joins"] == int(joins)]
        return stat(qs, name)
    if kind == "qerr_exact":  # stat table q: as qerr at 0 joins, with that table's prefixes off by q or more taken as estimated exactly
        name, table, q = args
        qs = [1.0 if p["table"] == table and p["q"] >= float(q) else p["q"] for l in lines.values() for p in l.get("prefixes", []) if p["joins"] == 0]
        return stat(qs, name)
    if kind == "largest":  # selector stat: the largest intermediate result's q-error, a query each
        selector, name = args
        return stat([l["largest_q"] for l in chosen(lines, selector).values()], name)
    if kind == "versus":  # engine selector geomean|queries|faster
        engine, selector, name = args
        g, n, faster = geomean_over_amphora(chosen(lines, selector), engine)
        return {"geomean": g, "queries": n, "faster": faster}[name]
    if kind == "share":  # selector: the selected queries' share of Amphora's time over all of them
        (selector,) = args
        return sum(l["ms"]["amphora"] for l in chosen(lines, selector).values()) / sum(l["ms"]["amphora"] for l in lines.values())
    if kind == "query":  # query field [engine]: ms, max_q, largest_q, plan_ms, tables
        query, field, *engine = args
        line = lines[query]
        return line["ms"][engine[0]] if field == "ms" else line[field]
    if kind == "prefix":  # query plan position est|act|q
        query, plan, position, field = args
        return next(p[field] for p in lines[query]["prefixes"] if p["plan"] == int(plan) and p["position"] == int(position))
    if kind == "plan_ms":  # stat over the queries' planning times
        (name,) = args
        return stat([l["plan_ms"] for l in lines.values()], name)
    if kind == "first":  # q table|any: queries whose first table (plan 0, position 0) is off by q or more
        q, table = args
        return sum(1 for l in lines.values() for p in l["prefixes"] if p["plan"] == 0 and p["position"] == 0 and p["q"] >= float(q) and table in ("any", p["table"]))
    if kind == "under":  # prefixes estimated below the rows they counted
        return sum(1 for l in lines.values() for p in l.get("prefixes", []) if p["est"] < p["act"])
    if kind == "below":  # est act prefixes|queries: prefixes estimated below est rows while counting act or more
        est, act, name = args
        hit = [(n, p) for n, l in lines.items() for p in l.get("prefixes", []) if p["est"] < float(est) and p["act"] >= float(act)]
        return len(hit) if name == "prefixes" else len({n for n, _ in hit})
    if kind == "inner_scans":  # count|est_min|est_max|act_max over the prefixes before them
        (name,) = args
        found = inner_scans(lines)
        return {"count": len(found), "est_min": min((p["est"] for _, p in found), default=0), "est_max": max((p["est"] for _, p in found), default=0),
                "act_max": max((p["act"] for _, p in found), default=0)}[name]
    if kind == "spearman":
        return spearman(lines)
    if kind == "slowest":  # n engine: the engine's time summed over Amphora's n slowest queries
        n, engine = args
        slow = sorted(lines.values(), key=lambda l: -l["ms"]["amphora"])[: int(n)]
        return sum(l["ms"][engine] for l in slow)
    if kind == "total":  # engine: its time summed over every query
        (engine,) = args
        return sum(l["ms"][engine] for l in lines.values())
    raise ValueError(f"no such kind: {kind}")


def job_text(directory):
    """For each JOB query: its terms equating two tables' columns, the terms beyond those that connect
    each class of columns they equate (a class of k columns needs k - 1), and the connected subsets of
    its join graph (a table a node, such a term an edge)."""
    out = {}
    for path in sorted(pathlib.Path(directory).glob("*.sql")):
        if not re.fullmatch(r"\d+[a-z]", path.stem):
            continue
        sql = path.read_text()
        tables = re.findall(r"\bAS\s+(\w+)", re.search(r"FROM(.*?)WHERE", sql, re.S | re.I).group(1), re.I)
        terms = [t for t in re.findall(r"\b(\w+)\.(\w+)\s*=\s*(\w+)\.(\w+)", sql) if t[0] != t[2]]
        parent = {}

        def find(x):
            parent.setdefault(x, x)
            while parent[x] != x:
                parent[x] = parent[parent[x]]
                x = parent[x]
            return x

        for a, ca, b, cb in terms:
            ra, rb = find((a, ca)), find((b, cb))
            if ra != rb:
                parent[ra] = rb
        classes = len({find(c) for c in list(parent)})
        index = {t: i for i, t in enumerate(tables)}
        adjacent = [0] * len(tables)
        for a, _, b, _ in terms:
            adjacent[index[a]] |= 1 << index[b]
            adjacent[index[b]] |= 1 << index[a]
        connected = 0
        for mask in range(1, 1 << len(tables)):
            seen = frontier = mask & -mask
            while frontier:
                reach = 0
                for i in range(len(tables)):
                    if frontier >> i & 1:
                        reach |= adjacent[i]
                frontier = reach & mask & ~seen
                seen |= frontier
            connected += seen == mask
        out[path.stem] = {"terms": len(terms), "surplus": len(terms) - (len(parent) - classes), "connected": connected}
    return out


def written(value, fmt):
    if fmt == "summary":
        return "" if value is None else (f"{value:.3g}" if value < 1e4 else f"{value:.3e}")
    if fmt == "pct":
        return f"{100 * value:.0f}%"
    return format(value, fmt)


def main(argv):
    job_dir = argv[argv.index("--job") + 1] if "--job" in argv else None
    record_text = RECORD.read_text()
    loaded, lines, rows, differ, text_rows, text_differ = {}, {}, 0, 0, 0, 0
    jobs = job_text(job_dir) if job_dir else None
    with open(HERE / "cited.tsv") as f:
        header = f.readline().rstrip("\n").split("\t")
        for line in f:
            row = dict(zip(header, line.rstrip("\n").split("\t")))
            args = row["how"].split()
            if row["records"] == "job-text":
                text_rows += 1
                if jobs is None:
                    continue
                query, field = args
                value = sum(1 for q in jobs.values() if q["surplus"] > 0) if query == "queries-with-surplus" else jobs[query][field]
                if written(value, row["format"]) != row["number"]:
                    text_differ += 1
                    print(f"differs: {row['number']} ({row['what']}): JOB's text gives {written(value, row['format'])}")
                continue
            rows += 1
            passes, bench, kind = tuple(row["records"].split(",")), args[0].replace("_", " "), args[1]
            if passes not in loaded:
                loaded[passes] = load(passes)
            if (passes, bench) not in lines:
                lines[(passes, bench)] = per_query(loaded[passes], bench)
            value = compute(kind, args[2:], lines[(passes, bench)])
            got = written(value, row["format"])
            if got != row["number"]:
                differ += 1
                print(f"differs: {row['number']} ({row['what']}): the records give {got}")
            elif row["in_record"] == "yes" and row["number"] not in record_text:
                differ += 1
                print(f"differs: {row['number']} ({row['what']}): not in {RECORD.name}'s text")
    text = f"; {text_rows} from JOB's text, {text_differ} differ" if jobs is not None else f"; {text_rows} from JOB's text not checked (no --job)"
    print(f"cited.py: {rows} numbers from step 3's records, {differ} differ{text}")
    return 1 if differ or text_differ else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
