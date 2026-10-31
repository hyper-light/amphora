#!/usr/bin/env python3
"""The correctness corpus (G2 step 2): each source run on SQLite, every statement tagged with the
inventory entries it exercises and its shape (tagger.py), and the totals the step asks for: distinct
query types (a shape with a set of features) and distinct statements per inventory entry.

Sources (sources.json names each repository and the revision read):
  slt         SQLite's sqllogictest, whole: each file run on SQLite as its own runner does
              (slt.py), its answers checked against the file's; disagreements kept for triage
  duckdb      DuckDB's test files, translated (translated.py); each kept file written under
              <out dir>/duckdb/ as a sqllogictest file with SQLite's answers
  cockroach   CockroachDB's logic tests, likewise, under <out dir>/cockroach/
  postgres    PostgreSQL's regression scripts, on its suite's base tables (test_setup.sql and its
              data files) made in SQLite; with CORPUS_PG_DSN set, each statement SQLite runs is run
              as written on that PostgreSQL too and the answers compared
  kiln        kiln's statements (kiln_queries.py), <checkout>/kiln/*.test
  written     the written cases (written_cases.py), <checkout>/written/**/*.test: each case counts
              for the entry it was written for (written/index.json) as well as what its text shows
  replay-duckdb, replay-cockroach, replay-postgres
              the files a translated source wrote (<checkout>/<source>/), run back on SQLite: every
              answer they carry is SQLite's own (PostgreSQL's after their fixture, base.test)

A statement counts once per entry however often it appears (sqllogictest repeats its CREATE TABLEs
in hundreds of files): each entry keeps the hashes of up to CAP distinct statements, so sources merge
(aggregate.py) without counting a statement twice.

usage: build_corpus.py <source> <source checkout> <features.json> <out dir> [--jobs N]
  writes <out dir>/<source>.json: per-file counts, disagreements, statements per entry, query types
"""
import collections
import hashlib
import json
import multiprocessing
import os
import resource
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import slt  # noqa: E402
import tagger as tagmod  # noqa: E402
import translated  # noqa: E402

SOURCE, ROOT, FEATURES, OUT = sys.argv[1:5]
JOBS = int(sys.argv[sys.argv.index("--jobs") + 1]) if "--jobs" in sys.argv else 4
CAP = 256  # distinct statements kept per entry: enough to tell ten from fewer, and to report "256+"
WORKER_MEMORY = 1536 << 20  # address space a file's process may take: 4 under the job's 6 GiB cap
STALL = 900  # seconds without a file finishing before the run names the files still out and stops
TAGGER = None
WRITTEN = None  # written/index.json: file -> the entry its cases were written for, and their first line


def type_key(feats, shape):
    """A query type: its shape and the entries it exercises, keywords aside (the syntax they make is
    already there)."""
    return json.dumps([list(shape), sorted(x for x in feats if not x.startswith("keyword:"))])


def text_hash(sql):
    return int.from_bytes(hashlib.blake2b(" ".join(sql.split()).encode(), digest_size=8).digest(), "big")


class Tally:
    """One file's statements: entries' occurrences, their distinct statements (up to CAP hashes), and
    query types with a count and a first example."""

    def __init__(self, rel):
        global TAGGER
        if TAGGER is None:
            TAGGER = tagmod.Tagger(FEATURES)
        self.rel, self.occ, self.distinct, self.types = rel, collections.Counter(), {}, {}

    def add(self, sql, line, credit=None):
        feats, shape = TAGGER.tag(sql)
        if credit:
            feats.add(credit)
        h = text_hash(sql)
        self.occ.update(feats)
        for e in feats:
            s = self.distinct.setdefault(e, set())
            if len(s) < CAP:
                s.add(h)
        k = type_key(feats, shape)
        if k in self.types:
            self.types[k][0] += 1
        else:
            self.types[k] = [1, f"{self.rel}:{line}"]

    def result(self, counts, off, n_off):
        return self.rel, counts, off, n_off, dict(self.occ), {e: sorted(s) for e, s in self.distinct.items()}, self.types


def run_slt_file(path):
    global WRITTEN
    rel = os.path.relpath(path, ROOT)
    if SOURCE.startswith("replay-"):  # answers checked only: the source's own run tagged them
        fixture = os.path.join(ROOT, "postgres", "base.test") if SOURCE == "replay-postgres" else None
        counts, off = slt.run_file(path, fixture=fixture)
        return rel, counts, [list(o) for o in off[:200]], len(off), {}, {}, {}
    credit = None
    if SOURCE == "written":
        if WRITTEN is None:
            WRITTEN = json.load(open(os.path.join(ROOT, "written", "index.json")))
        credit = WRITTEN[rel]
    tally = Tally(rel)

    def keep(rec, got):
        tally.add(rec.sql, rec.line, credit["entry"] if credit and rec.line >= credit["from_line"] else None)

    counts, off = slt.run_file(path, keep=keep)
    return tally.result(counts, [list(o) for o in off[:200]], len(off))


PG_BASE = None  # (base database, setup statements, COPY sources), made before the workers start


def start_worker(base):
    """Each file's process: the PostgreSQL base, and memory bounded so a runaway answer is an error
    its record takes (SQLite's "out of memory", Python's MemoryError), not the kernel's OOM kill
    that loses the file and holds the run."""
    global PG_BASE
    PG_BASE = base
    resource.setrlimit(resource.RLIMIT_AS, (WORKER_MEMORY, WORKER_MEMORY))


def run_translated_file(path):
    rel = os.path.relpath(path, ROOT)
    tally = Tally(rel)

    def keep(rec, sql):
        tally.add(sql, rec["line"])

    if SOURCE == "postgres":
        base_db, setup, copies = PG_BASE
        counts, off, text = translated.run_postgres(path, os.path.join(ROOT, "src", "test", "regress"), base_db, setup, copies, os.environ.get("CORPUS_PG_DSN"), keep)
        if counts["kept"]:
            dest = os.path.join(OUT, SOURCE, rel.replace(".sql", ".test"))
            os.makedirs(os.path.dirname(dest), exist_ok=True)
            open(dest, "w").write(text)
        return tally.result(counts, off, counts["disagree"])
    parse, read = {"duckdb": (translated.parse_duckdb, "duckdb"), "cockroach": (translated.parse_cockroach, "postgres")}[SOURCE]
    records = parse(path)
    if records is None:
        return tally.result({"files_skipped": 1}, [], 0)
    counts, off, text = translated.run(path, records, read, keep)
    if counts["kept"]:
        dest = os.path.join(OUT, SOURCE, rel)
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        open(dest if dest.endswith(".test") else dest + ".test", "w").write(text)
    return tally.result(counts, off, counts["disagree"])


def walk(top, suffixes):
    out = []
    for d, _, files in os.walk(top):
        out += [os.path.join(d, f) for f in files if f.endswith(suffixes)]
    return sorted(out)


def postgres_files():
    d = os.path.join(ROOT, "src", "test", "regress", "sql")
    return sorted(os.path.join(d, f) for f in os.listdir(d) if f.endswith(".sql") and f != "test_setup.sql")


def cockroach_files():
    d = os.path.join(ROOT, "pkg", "sql", "logictest", "testdata", "logic_test")
    return sorted(os.path.join(d, f) for f in os.listdir(d) if not f.startswith("."))


FILES = {
    "slt": lambda: walk(os.path.join(ROOT, "test"), (".test",)),
    "duckdb": lambda: walk(os.path.join(ROOT, "test", "sql"), (".test", ".test_slow")),
    "cockroach": cockroach_files,
    "postgres": postgres_files,
    "kiln": lambda: walk(os.path.join(ROOT, "kiln"), (".test",)),
    "written": lambda: walk(os.path.join(ROOT, "written"), (".test",)),
    "replay-duckdb": lambda: walk(os.path.join(ROOT, "duckdb"), (".test",)),
    "replay-cockroach": lambda: walk(os.path.join(ROOT, "cockroach"), (".test",)),
    "replay-postgres": lambda: [p for p in walk(os.path.join(ROOT, "postgres"), (".test",)) if os.path.basename(p) != "base.test"],
}


def main():
    global PG_BASE
    files = FILES[SOURCE]()
    if SOURCE == "postgres":
        os.makedirs(OUT, exist_ok=True)
        base_db = os.path.join(OUT, "postgres-base.db")
        if os.path.exists(base_db):
            os.remove(base_db)
        setup, copies, fixture = translated.pg_base(os.path.join(ROOT, "src", "test", "regress"), base_db)
        PG_BASE = (base_db, setup, copies)
        os.makedirs(os.path.join(OUT, "postgres"), exist_ok=True)
        open(os.path.join(OUT, "postgres", "base.test"), "w").write(fixture)
        if os.environ.get("CORPUS_PG_DSN"):
            made = translated.pg_template(os.path.join(ROOT, "src", "test", "regress"), os.environ["CORPUS_PG_DSN"], setup, copies)
            print(f"PostgreSQL's base tables: {made} of {len(setup)} setup statements ran", flush=True)
    worker = run_slt_file if SOURCE in ("slt", "kiln", "written") or SOURCE.startswith("replay-") else run_translated_file
    totals, occ, distinct, types = collections.Counter(), collections.Counter(), {}, {}
    report = {"source": SOURCE, "files": {}}
    # Each file in a process of its own, as the C runner runs them: a file's process-wide settings
    # (heap limits) don't reach the next. The processes fork from a server with the modules loaded.
    ctx = multiprocessing.get_context("forkserver")
    ctx.set_forkserver_preload(["slt", "tagger", "translated", "sqlglot"])
    out = {os.path.relpath(f, ROOT) for f in files}
    with ctx.Pool(JOBS, initializer=start_worker, initargs=(PG_BASE,), maxtasksperchild=1) as pool:
        results = pool.imap_unordered(worker, files)
        while True:
            try:
                rel, counts, off, n_off, f_occ, f_distinct, f_types = results.next(timeout=STALL)
            except StopIteration:
                break
            except multiprocessing.TimeoutError:
                # a file's process died without an answer (killed): the pool would wait for it forever
                print(f"no file finished in {STALL} s; still out: {sorted(out)}", flush=True)
                sys.exit(1)
            out.discard(rel)
            totals.update(counts)
            occ.update(f_occ)
            for e, hashes in f_distinct.items():
                s = distinct.setdefault(e, set())
                for h in hashes:
                    if len(s) >= CAP:
                        break
                    s.add(h)
            for k, (c, ex) in f_types.items():
                if k in types:
                    types[k][0] += c
                else:
                    types[k] = [c, ex]
            report["files"][rel] = {"counts": counts, "disagreements": n_off, "first_disagreements": off}
            print(f"{rel}: " + ", ".join(f"{k} {v}" for k, v in counts.items() if v), flush=True)
    report["totals"] = dict(totals)
    report["per_entry"] = dict(occ)
    report["per_entry_distinct"] = {e: len(s) for e, s in distinct.items()}
    report["per_entry_hashes"] = {e: sorted(s) for e, s in distinct.items()}
    report["types"] = types
    os.makedirs(OUT, exist_ok=True)
    json.dump(report, open(os.path.join(OUT, f"{SOURCE}.json"), "w"))
    print(json.dumps(dict(totals)), f"{len(types)} query types, {len(distinct)} entries exercised")


if __name__ == "__main__":
    main()
