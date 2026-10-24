#!/usr/bin/env python3
"""The corpus's totals (G2 step 2): every source's report (build_corpus.py) merged. Distinct query
types across all of them (a shape with a set of features: the same type from two sources counts
once); distinct statements per inventory entry (each source keeps up to 256 statement hashes an
entry, unioned here, so a statement two sources share counts once); the entries reached fewer than
ten times; and each source's records, agreement and disagreements. Writes corpus.json, corpus.md
and files-<source>.tsv (each file's counts) to the record's directory.

usage: aggregate.py <features.json> <corpus dir> <record dir> [<sources.json>] [<triage.json>]
  The translated sources' disagreements are triaged as they ran (translated.classify: each counted
  under its cause); triage.json explains the rest by hand: for a source, its causes, each with a
  count, why, and an example. The replays (each translated source's files run back on SQLite) are
  reported beside the sources.
"""
import collections
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from translated import WHY  # noqa: E402

FEATURES, CORPUS, RECORD = sys.argv[1:4]
SOURCES_JSON = sys.argv[4] if len(sys.argv) > 4 else None
TRIAGE_JSON = sys.argv[5] if len(sys.argv) > 5 else None
SOURCES = ["slt", "duckdb", "cockroach", "postgres", "kiln", "written"]
NEED, CAP = 10, 256


def main():
    feats = json.load(open(FEATURES))["features"]
    ids = [f["id"] for f in feats]
    kind_of = {f["id"]: f["kind"] for f in feats}
    reports = {s: json.load(open(os.path.join(CORPUS, f"{s}.json"))) for s in SOURCES if os.path.exists(os.path.join(CORPUS, f"{s}.json"))}
    types, type_sources = {}, collections.defaultdict(set)
    hashes, occurrences = collections.defaultdict(set), collections.Counter()
    per_source = {}
    for s, r in reports.items():
        for k, (n, example) in r["types"].items():
            if k in types:
                types[k][0] += n
            else:
                types[k] = [n, f"{s}:{example}"]
            type_sources[k].add(s)
        for e, hs in r.get("per_entry_hashes", {}).items():
            hashes[e].update(hs)
        occurrences.update(r.get("per_entry", {}))
        t = r["totals"]
        per_source[s] = {
            "files": len(r["files"]), "totals": t, "query_types": len(r["types"]), "entries_reached": len(r.get("per_entry_distinct", {})),
            "disagreements": sum(f["disagreements"] for f in r["files"].values()),
            "files_with_disagreements": sum(1 for f in r["files"].values() if f["disagreements"]),
        }
    replays = {}
    for s in ("duckdb", "cockroach", "postgres"):
        path = os.path.join(CORPUS, f"replay-{s}.json")
        if os.path.exists(path):
            r = json.load(open(path))
            settled = os.path.join(CORPUS, f"settled-{s}.json")
            replays[s] = {"files": len(r["files"]), "totals": r["totals"], "settled": json.load(open(settled)) if os.path.exists(settled) else None,
                          "disagreements": [[rel] + d for rel, f in r["files"].items() for d in f["first_disagreements"]][:40]}
    triage = {}
    for s, r in reports.items():  # what each translated source's run counted under each cause
        causes = sorted(((k[5:], v) for k, v in r["totals"].items() if k.startswith("why: ")), key=lambda kv: -kv[1])
        if causes:
            examples = {}
            for rel, f in r["files"].items():
                for d in f["first_disagreements"]:
                    if len(d) > 1 and isinstance(d[1], str) and d[1] in WHY:
                        examples.setdefault(d[1], f"{rel}:{d[0]}")
            triage[s] = [{"cause": c, "count": n, "why": WHY.get(c, ""), "example": examples.get(c)} for c, n in causes]
    if TRIAGE_JSON:
        for s, causes in json.load(open(TRIAGE_JSON)).items():
            if s != "notes":
                triage.setdefault(s, []).extend(causes)
    for s in per_source:  # types no other source has (counted once every source is read)
        per_source[s]["only_here"] = sum(1 for k in reports[s]["types"] if type_sources[k] == {s})
    distinct = {e: len(hashes.get(e, ())) for e in ids}
    short = sorted((distinct[e], e) for e in ids if distinct[e] < NEED)
    by_kind = collections.OrderedDict()
    for e in ids:
        k = kind_of[e]
        row = by_kind.setdefault(k, {"entries": 0, "reached_10": 0, "median": []})
        row["entries"] += 1
        row["reached_10"] += distinct[e] >= NEED
        row["median"].append(distinct[e])
    for row in by_kind.values():
        v = sorted(row["median"])
        row["median"] = v[len(v) // 2]
        row["least"] = v[0]
    shape_kinds = collections.Counter(json.loads(k)[0][0] for k in types)
    # A coarser count beside the step's: a statement kind with the syntax, functions, operators,
    # PRAGMAs and date modifiers it uses (the shape's other dimensions, literals' storage classes,
    # rules and words aside), so the headline doesn't rest on literal kinds.
    coarse_kinds = ("syntax:", "function.", "operator:", "pragma:", "datetime.modifier:")
    coarse = {(json.loads(k)[0][0], tuple(f for f in json.loads(k)[1] if f.startswith(coarse_kinds))) for k in types}
    out = {
        "query_types": len(types), "coarse_query_types": len(coarse), "entries": len(ids), "entries_reached_10": len(ids) - len(short), "coverage_10": round(100.0 * (len(ids) - len(short)) / len(ids), 2),
        "short": short, "per_source": per_source, "by_kind": by_kind, "types_by_statement_kind": dict(shape_kinds.most_common()),
        "per_entry_distinct": distinct, "per_entry_occurrences": {e: occurrences.get(e, 0) for e in ids},
        "first_disagreements": {s: [[rel] + d for rel, f in r["files"].items() for d in f["first_disagreements"][:3]][:40] for s, r in reports.items()},
        "sources": json.load(open(SOURCES_JSON)) if SOURCES_JSON else None,
        "replays": replays, "triage": triage,
    }
    os.makedirs(RECORD, exist_ok=True)
    json.dump(out, open(os.path.join(RECORD, "corpus.json"), "w"), indent=1)
    for s, r in reports.items():  # each file's counts, for the steps that run the corpus on every engine
        keys = sorted({k for f in r["files"].values() for k in f["counts"]})
        rows = ["\t".join(["file"] + keys + ["disagreements"])]
        rows += ["\t".join([rel] + [str(f["counts"].get(k, 0)) for k in keys] + [str(f["disagreements"])]) for rel, f in sorted(r["files"].items())]
        open(os.path.join(RECORD, f"files-{s}.tsv"), "w").write("\n".join(rows) + "\n")
    md = ["# The correctness corpus, run on SQLite", "",
          f"**{len(types):,} distinct query types** (a statement's shape with the inventory entries it exercises, keywords aside); "
          f"**{len(ids) - len(short)} of {len(ids)} inventoried entries ({out['coverage_10']}%) exercised by at least {NEED} distinct statements**. "
          f"Counted coarser (a statement kind with the syntax, functions, operators, PRAGMAs and date modifiers it uses), {len(coarse):,} types.", "",
          "| source | files | records kept | answers compared | agree | disagree | query types | of them only here | entries reached |", "|---|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for s, p in per_source.items():
        t = p["totals"]
        kept = t["kept"] if "kept" in t else t.get("statements", 0) + t.get("queries", 0)
        md.append(f"| {s} | {p['files']:,} | {kept:,} | {t.get('agree', 0) + t.get('disagree', 0):,} | {t.get('agree', 0):,} | {t.get('disagree', 0):,} | {p['query_types']:,} | {p['only_here']:,} | {p['entries_reached']} |")
    md += ["", "For slt, kiln and written, `agree` is SQLite's answer against the one the file carries (sqllogictest's own; SQLite's for the others, so "
           "those agreeing say the files replay). For the translated sources (duckdb, cockroach, postgres) it's SQLite's answer against the source engine's: "
           "their records are what was read, what sqlglot couldn't translate, what SQLite refused, and what was kept. A record both engines refuse is kept as "
           "`statement error` (an error path is a case too): the few hundred query types under statement kinds SQLite doesn't have (COPY, CALL, PREPARE) are those.", "",
           "| source | records | untranslatable | SQLite refused | kept | kept partly | not compared | files left out |", "|---|---:|---:|---:|---:|---:|---:|---:|"]
    for s in ("duckdb", "cockroach", "postgres"):
        if s in per_source:
            t = per_source[s]["totals"]
            md.append(f"| {s} | {t.get('records', 0):,} | {t.get('untranslatable', 0):,} | {t.get('sqlite_refused', 0):,} | {t.get('kept', 0):,} | {t.get('kept_partly', 0):,} | {t.get('not_compared', 0):,} | {t.get('files_skipped', 0):,} |")
    md += ["", "## Entries by kind", "", f"| kind | entries | reached by {NEED}+ | least | median |", "|---|---:|---:|---:|---:|"]
    for k, row in by_kind.items():
        md.append(f"| {k} | {row['entries']} | {row['reached_10']} | {row['least']}{'+' if row['least'] >= CAP else ''} | {row['median']}{'+' if row['median'] >= CAP else ''} |")
    md += ["", f"Distinct statements an entry are counted up to {CAP} a source ({CAP}+ means at least that many).", ""]
    if short:
        md += [f"## Entries reached fewer than {NEED} times", ""] + [f"- `{e}`: {n}" for n, e in short] + [""]
    md += ["## Query types by statement kind", "", "| kind | query types |", "|---|---:|"] + [f"| {k or '(none)'} | {n:,} |" for k, n in shape_kinds.most_common(25)] + [""]
    src = out["sources"] or {}
    if src:
        md += ["## How it ran", "",
               f"SQLite {src['oracle']['version']} the oracle ({src['oracle']['through']}); translation by sqlglot {src['translator']['version']}. "
               "Each file runs on a fresh database file as slt_sqlite.c runs it: synchronous off; an I column is sqlite3_column_int (32 bits), an R column "
               "SQLite's own printf(\"%.3f\"), a T column the text's bytes up to the first NUL with each outside ' '..'~' an @; rowsort and valuesort as "
               "its runner sorts. Each file in a process of its own (no process-wide setting reaches the next), within budgets: 20 s and a million rows "
               "a record, 1.5 GiB a process. A query answering from random() or the clock is kept as a statement (run, its answer not checked).", "",
               "| source | repository | revision | read |", "|---|---|---|---|"]
        for s, d in src["sources"].items():
            md.append(f"| {s} | {d['repository']} | {d.get('revision', '')[:12]} | {d['read']} |")
        md.append("")
    for note in (json.load(open(TRIAGE_JSON)).get("notes", []) if TRIAGE_JSON else []):
        md += [note, ""]
    if replays:
        md += ["## Every expected result SQLite's own", "", "Each translated source's files run back on SQLite (`build_corpus.py replay-<source>`): the answers they carry are the ones SQLite gives. "
               "A query whose answer moved between the run that wrote it and its replay answers from the clock or chance through its data (PostgreSQL's timestamp "
               "tests store 'now', which SQLite's date functions read as the time): `settle.py` keeps it as a statement and the replay runs again.", "",
               "| source | files | statements | queries | agree | disagree | queries settled |", "|---|---:|---:|---:|---:|---:|---:|"]
        for s, r in replays.items():
            t = r["totals"]
            md.append(f"| {s} | {r['files']:,} | {t.get('statements', 0):,} | {t.get('queries', 0):,} | {t.get('agree', 0):,} | {t.get('disagree', 0):,} | {(r['settled'] or {}).get('settled', 0):,} |")
        md.append("")
    if triage:
        md += ["## Disagreements, triaged", "", "For the translated sources, SQLite's answer against the source engine's own (DuckDB's and CockroachDB's expected results, "
               "PostgreSQL 17 run live), each counted under the first cause that fits it (`translated.classify`).", ""]
        for s, causes in triage.items():
            md += [f"### {s}", "", "| cause | count | what | example |", "|---|---:|---|---|"]
            for c in causes:
                md.append(f"| {c['cause']} | {c['count']:,} | {c['why']} | {('`' + c['example'] + '`') if c.get('example') else ''} |")
            md.append("")
    open(os.path.join(RECORD, "corpus.md"), "w").write("\n".join(md))
    print(f"{len(types)} query types; {len(ids) - len(short)}/{len(ids)} entries reached {NEED}+ ({out['coverage_10']}%); short: {len(short)}")
    for s, p in per_source.items():
        print(f"  {s}: {p['files']} files, {p['query_types']} types ({p['only_here']} only here), {p['disagreements']} disagreements, totals {p['totals']}")


main()
