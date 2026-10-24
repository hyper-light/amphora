#!/usr/bin/env python3
"""The support matrix (G2 step 1): every entry of the SQLite language's inventory against every
engine the bench reaches, from the probes' answers (probe.py, `amphora-bench --probe`), SQLite's
answers the reference. For each entry and engine: whether it answers as SQLite does, and how that
engine spells it (SQLite's own words, a hand-written spelling from spellings.json, or sqlglot's
translation), or why it can't.

An entry's status in an engine:
  same          every probe of it answers as SQLite does (a refusal SQLite expects counts)
  partial       some of its probes do, some don't
  same values, other form  the same values written otherwise: text '1' for integer 1, a boolean
                for 1, a JSON document with its keys or spaces otherwise
  differs       an answer other than SQLite's
  error         the engine refuses it (unsupported, or it fails)
  crash         the engine's server died under it
  untranslatable  sqlglot can't say it in the engine's dialect and no rule spells it
  no equivalent the engine has no way to say it (a rule says why)
  not applicable  a client-side parameter syntax: each driver binds its own
  no reference  SQLite's build can't answer it (a compile-time option it lacks): the engine's own
                outcome is shown as ran or error
Keywords have no probes of their own: a keyword's status counts the probes whose statements use it.

usage: matrix.py <features.json> <probes.json> <results dir> <out matrix.json> <out matrix.md>
  results dir: <engine>.json for sqlite and each engine (missing engines are left out).
"""
import collections
import json
import math
import os
import re
import sys

FEATURES, PROBES, RESULTS, OUT_JSON, OUT_MD = sys.argv[1:6]
ENGINES = ["amphora", "turso", "duckdb", "postgres", "orioledb", "cedardb", "mysql", "clickhouse"]
LABEL = {"amphora": "Amphora", "turso": "Turso", "duckdb": "DuckDB", "postgres": "PostgreSQL", "orioledb": "OrioleDB", "cedardb": "CedarDB", "mysql": "MySQL", "clickhouse": "ClickHouse"}
ORDER = ["same", "partial", "same values, other form", "differs", "error", "crash", "untranslatable", "no equivalent", "not applicable", "no reference"]

feats = json.load(open(FEATURES))
probes = json.load(open(PROBES))["probes"]
res = {}
meta = {}
for e in ["sqlite"] + ENGINES:
    path = os.path.join(RESULTS, f"{e}.json")
    if os.path.exists(path):
        r = json.load(open(path))
        res[e] = {x["id"]: x for x in r["results"]}
        meta[e] = {k: r.get(k) for k in ("version", "sqlglot", "seconds", "crashes")}
engines = [e for e in ENGINES if e in res]


def norm(v):
    """One spelling per value, as probe.py writes them (the Rust runner's floats are unrounded)."""
    if isinstance(v, bool):
        return int(v)
    if isinstance(v, float):
        if math.isnan(v):
            return "NaN"
        if v == int(v) and abs(v) < 2**53:
            return int(v)
        return float(f"{v:.9g}")
    return v


def loose(v):
    """A value with its form set aside: numbers and numeric text alike, booleans as numbers, a JSON
    document as its value (its keys in order, its spaces gone)."""
    v = norm(v)
    if isinstance(v, str):
        try:
            return norm(float(v))
        except ValueError:
            pass
        if v.lower() in ("true", "false"):
            return int(v.lower() == "true")
        if v[:1] in "[{":
            try:
                return json.dumps(json.loads(v), sort_keys=True, separators=(",", ":"))
            except ValueError:
                pass
    return v


def rows_of(r, ordered):
    rows = [[norm(v) for v in row] for row in r.get("rows", [])]
    return rows if ordered else sorted(rows, key=lambda x: json.dumps(x, sort_keys=True))


def compare(p, ref, got):
    """One probe in one engine against SQLite's answer."""
    if got is None:
        return "error", "no result"
    st = got["status"]
    if st == "n/a":
        return "not applicable", got.get("error")
    if st in ("untranslatable", "no equivalent", "crash"):
        return st, got.get("error")
    if ref is None or (ref["status"] != "ok" and p.get("expect") != "error"):
        return "no reference", ("ran" if st == "ok" else "error: " + (got.get("error") or "")[:160])
    if p.get("expect") == "error":
        return ("same", "refused, as SQLite does") if st != "ok" else ("differs", "accepted what SQLite refuses")
    if st != "ok":
        return "error", (got.get("error") or "")[:200]
    a, b = rows_of(ref, p["ordered"]), rows_of(got, p["ordered"])
    # Answers each implementation words its own way (EXPLAIN's program, a build's lists, a file's
    # path): the engine answers as SQLite does when it answers at all, or in the same shape.
    mode = p.get("compare", "exact")
    if mode == "nonempty":
        return ("same", None) if b else ("differs", "no rows, where SQLite answers")
    if mode == "shape":
        same_shape = bool(b) and len(a) == len(b) and all(len(x) == len(y) for x, y in zip(a, b))
        return ("same", None) if same_shape else ("differs", f"SQLite {len(a)} rows of {len(a[0]) if a else 0}, here {len(b)} rows of {len(b[0]) if b else 0}")
    if a == b:
        return "same", None
    la = [[loose(v) for v in r] for r in a]
    lb = [[loose(v) for v in r] for r in b]
    if not p["ordered"]:
        la, lb = sorted(la, key=json.dumps), sorted(lb, key=json.dumps)
    if la == lb:
        return "same values, other form", f"SQLite {json.dumps(a)[:120]}, here {json.dumps(b)[:120]}"
    return "differs", f"SQLite {json.dumps(a)[:120]}, here {json.dumps(b)[:120]}"


def spelling(e, got):
    if got is None:
        return None
    if e in ("amphora", "turso"):
        return {"how": "SQLite's own", "sql": None}
    how = got.get("spelling")
    if got["status"] in ("untranslatable", "no equivalent"):
        return {"how": got["status"], "why": got.get("error")}
    if got["status"] == "n/a":
        return {"how": "its driver's parameter syntax", "sql": None}
    return {"how": {"rule": "hand-written", "sqlglot": "sqlglot", "sqlite": "SQLite's own"}.get(how, how), "sql": got.get("sql")}


by_entry = collections.defaultdict(list)
for p in probes:
    by_entry[p["id"].split("#")[0]].append(p)


def words(sql):
    return {w.upper() for w in re.findall(r"[A-Za-z_][A-Za-z_0-9]*", re.sub(r"'(?:[^']|'')*'", " ", sql))}


probe_words = {p["id"]: set().union(*(words(s) for s in p["setup"] + [p["query"]])) for p in probes}

entries = []
for f in feats["features"]:
    row = {"id": f["id"], "kind": f["kind"], "name": f["name"], "doc": f["doc"], "engines": {}}
    if f["kind"] == "keyword":
        ps = [p for p in probes if f["name"] in probe_words[p["id"]]]
        row["probes"] = len(ps)
        for e in engines:
            outcomes = collections.Counter(compare(p, res["sqlite"].get(p["id"]), res[e].get(p["id"]))[0] for p in ps)
            same = outcomes["same"]
            status = "same" if same == len(ps) else ("partial" if same else (max(outcomes, key=lambda k: (outcomes[k], -ORDER.index(k))) if outcomes else "no reference"))
            # Its spelling: the word itself when the engine's statements still say it, else how
            # the first statement using it was put.
            spelled = [res[e].get(p["id"], {}).get("sql") or "" for p in ps]
            kept = sum(f["name"] in words(s) for s in spelled if s)
            sp = {"how": "SQLite's own" if e in ("amphora", "turso") else ("the same word" if kept else "said otherwise"), "statements": len(ps), "answer as SQLite does": same}
            if not kept and e not in ("amphora", "turso"):
                sp["example"] = next((s for s in spelled if s), None)
            row["engines"][e] = {"status": status, "outcomes": dict(outcomes), "spelling": sp}
    else:
        ps = by_entry.get(f["id"], [])
        row["probes"] = [p["id"] for p in ps]
        for e in engines:
            outs = [(p["id"],) + compare(p, res["sqlite"].get(p["id"]), res[e].get(p["id"])) for p in ps]
            kinds = [o[1] for o in outs]
            if kinds and all(k == "same" for k in kinds):
                status = "same"
            elif "same" in kinds:
                status = "partial"
            elif kinds:
                status = min(kinds, key=ORDER.index) if all(k in ("same values, other form", "differs") for k in kinds) else max(collections.Counter(kinds).items(), key=lambda kv: (kv[1], -ORDER.index(kv[0])))[0]
            else:
                status = "no reference"
            first = res[e].get(ps[0]["id"]) if ps else None
            row["engines"][e] = {"status": status, "detail": [{"probe": o[0], "outcome": o[1], "note": o[2]} for o in outs if o[1] != "same"], "spelling": spelling(e, first)}
    entries.append(row)

json.dump({"sqlite_doc": feats["sqlite_doc"], "engines": {e: meta.get(e) for e in ["sqlite"] + engines}, "entries": entries}, open(OUT_JSON, "w"), indent=1)

# ---------------------------------------------------------------- the published summary
kinds = list(dict.fromkeys(f["kind"] for f in feats["features"]))
lines = ["# The SQLite language against every engine (G2 step 1)", ""]
repo = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
where = os.path.relpath(os.path.abspath(RESULTS), repo)
answers = f" The probes' answers, engine by engine: `{where}/`." if not where.startswith("..") else ""
lines.append(f"Every entry of `features.json` ({len(entries)}, from SQLite's documentation {feats['sqlite_doc']}), probed on each engine with "
             f"`probes.json` ({len(probes)} probes) and compared with SQLite's own answers. Built by `bench/corpus/matrix.py`; each entry's per-engine "
             f"spelling and every disagreement are in `matrix.json`.{answers}")
lines.append("")
lines.append("## The inventory")
lines.append("")
lines.append("`inventory.py` reads SQLite's documentation; `check_inventory.py` holds the result against every list the documentation keeps of "
             "its own language and against SQLite's own catalog (pragma_function_list, pragma_pragma_list): nothing documented may be missing.")
lines.append("")
lines.append("| kind | entries |")
lines.append("|---|---:|")
for k, n in feats["counts"].items():
    lines.append(f"| {k} | {n} |")
lines.append(f"| all | {len(entries)} |")
check = os.path.join(RESULTS, "check_inventory.txt")
if os.path.exists(check):
    lines.append("")
    lines.append("```")
    lines += [l.rstrip() for l in open(check) if l.strip()]
    lines.append("```")
lines.append("")
lines.append("## Every engine against SQLite")
lines.append("")
lines.append("| engine | version | as SQLite (entries) |")
lines.append("|---|---|---:|")
lines.append(f"| SQLite (reference) | {meta['sqlite']['version']} | - |")
def short(v):
    """A version string to its first clause (PostgreSQL's names its compiler and platform)."""
    return re.split(r" on x86_64| on aarch64|, compiled by|, compiled on| PGTAG=", v or "-")[0]


for e in engines:
    same = sum(r["engines"][e]["status"] == "same" for r in entries)
    lines.append(f"| {LABEL[e]} | {short(meta[e].get('version'))} | {same} of {len(entries)} ({100 * same / len(entries):.0f}%) |")
lines.append("")
lines.append("**Entries answering as SQLite does, by kind** (a keyword counts once every statement using it answers as SQLite does):")
lines.append("")
lines.append("| kind | entries | " + " | ".join(LABEL[e] for e in engines) + " |")
lines.append("|---|---:|" + "---:|" * len(engines))
for k in kinds:
    rs = [r for r in entries if r["kind"] == k]
    lines.append(f"| {k} | {len(rs)} | " + " | ".join(str(sum(r["engines"][e]["status"] == "same" for r in rs)) for e in engines) + " |")
lines.append("")
lines.append("**Every entry's status, counted** (an entry's worst-represented outcome when its probes split):")
lines.append("")
lines.append("| status | " + " | ".join(LABEL[e] for e in engines) + " |")
lines.append("|---|" + "---:|" * len(engines))
for s in ORDER:
    counts = [sum(r["engines"][e]["status"] == s for r in entries) for e in engines]
    if any(counts):
        lines.append(f"| {s} | " + " | ".join(str(c) for c in counts) + " |")
lines.append("")
spell = collections.Counter()
for r in entries:
    if r["kind"] == "keyword":
        continue
    for e in engines:
        sp = r["engines"][e]["spelling"] or {}
        spell[(e, sp.get("how"))] += 1
lines.append("**How each engine's probes were spelled** (non-keyword entries):")
lines.append("")
hows = sorted({h for _, h in spell}, key=str)
lines.append("| spelling | " + " | ".join(LABEL[e] for e in engines) + " |")
lines.append("|---|" + "---:|" * len(engines))
for h in hows:
    lines.append(f"| {h} | " + " | ".join(str(spell[(e, h)]) for e in engines) + " |")
lines.append("")
for e in ["amphora", "turso"]:
    if e not in engines:
        continue
    gaps = [r for r in entries if r["kind"] != "keyword" and r["engines"][e]["status"] not in ("same", "no reference")]
    lines.append(f"## Where {LABEL[e]} doesn't answer as SQLite does ({len(gaps)} entries)")
    lines.append("")
    for r in gaps:
        d = r["engines"][e]["detail"]
        note = "; ".join(f"{x['probe'].split('#')[-1] if '#' in x['probe'] else ''}{' ' if '#' in x['probe'] else ''}{x['outcome']}: {(x['note'] or '')[:150]}" for x in d)
        lines.append(f"- `{r['id']}` ({r['engines'][e]['status']}): {note}")
    lines.append("")
crashes = [(e, r["id"]) for r in entries for e in engines if r["engines"][e]["status"] == "crash" or any(x["outcome"] == "crash" for x in r["engines"][e].get("detail", []) if isinstance(x, dict))]
if crashes:
    lines.append("## Probes that crashed a server")
    lines.append("")
    for e, i in crashes:
        lines.append(f"- {LABEL[e]}: `{i}`")
    lines.append("")
open(OUT_MD, "w").write("\n".join(lines))
print(f"{len(entries)} entries x {len(engines)} engines")
for e in engines:
    print(f"  {LABEL[e]:<11} same {sum(r['engines'][e]['status'] == 'same' for r in entries):4d}")
