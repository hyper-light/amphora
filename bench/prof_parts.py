#!/usr/bin/env python3
"""Each profiled statement's cycles by named part (G1 step 5): reads a perf record's frame-pointer
call graphs through `perf script`, puts every sample's leaf frame in a part by its symbol (Amphora's
Rust paths, SQLite's C functions), and weighs parts by the samples' periods. Kernel time isn't
sampled (perf_event_paranoid 2 here): each run's record gives it as cpu_sys_s, shown beside.

usage: python3 bench/prof_parts.py <dir of <target>-<statement>-<clients>.data and -core.json> [--top N]
  prints a markdown table per statement: each part's share for Amphora and SQLite; with --top, each
  engine's N leaf functions with the most cycles beside it.
"""
import collections
import json
import os
import re
import subprocess
import sys

# Leaf symbol patterns, first match wins. Amphora's (Rust) and SQLite's (C) names for the same part.
PARTS = [
    ("std hashing (SipHash)", r"RandomState|sip::Hasher|SipHasher|BuildHasher>::hash_one"),
    ("errors made and dropped", r"drop_glue::<.*Error>|LimboError"),
    ("hash tables (GROUP BY, DISTINCT, joins)", r"hash_table::|HashTable|insert_distinct|hash_join_key|keys_equal|values_equal"),
    ("JSON", r"json::|Jsonb|jsonb|JsonbHeader|sqlite3Json|jsonParse|jsonLookup|json[A-Z][a-z]+"),
    ("sorting", r"vdbeSorter|Sorter|sorter::|sqlite3VdbeSorter|sort_unstable|merge_sort|::sort"),
    ("aggregation", r"sumStep|countStep|avgStep|minmaxStep|kahanBabuskaNeumaierStep|AggStep|sqlite3VdbeMemFinalize|Accumulator|aggregate::|group_table|AggContext|op_agg"),
    ("statement: parse and plan", r"sqlite3RunParser|yy_reduce|yy_|where[A-Z][A-Za-z]+|resolveExpr|sqlite3Prepare|sqlite3Select[A-Z]|sqlite3Walk|findElementWithHash|translate::|parser::|optimizer::|sqlite3Expr"),
    ("checksums", r"xxh3|xxhash|Xxh3|XXH3|checksum|_mm256_|_mm_xor|hash_long"),
    ("atomics and reference counts", r"atomic_|Arc<.*>::drop|drop_slow|Arc::clone|fetch_add|fetch_sub|compare_exchange"),
    ("I/O completion", r"completions::|Completion|CompletionInner|OnceLock|call_once|ContextInner|IOCompletions|finish_pending_io"),
    ("write-ahead log: frame lookups and reads", r"wal::|find_frame|latest_frame|WalFile|WalCoordination|wal_page_reader|read_wal_frame|sqlite3WalFindFrame|walFramePage|walIndex|walTryBeginRead|unixShmBarrier|sqlite3WalRead"),
    ("page cache and pager", r"page_cache::|PageCache|pager::|Pager>?::|read_page|buffer_pool|BufferPool|Page>?::|pcache|Pcache|sqlite3PagerGet|getPageNormal|sqlite3PagerUnref|pagerUnlock|readDbPage|btreeBeginTrans"),
    ("file reads (user side)", r"pread|unixRead|seekAndRead|__libc_read|read_at"),
    ("B-tree search", r"btree::|BTreeCursor|BTree|move_to|seek|binary_search|compare_record|compare_payload|RecordCompare|sqlite3Btree|moveToChild|moveToRoot|sqlite3VdbeRecordCompare|vdbeRecordCompare|cellSize|btreeParseCell|getAndInitPage|getCellInfo"),
    ("record decoding", r"ValueIterator|serial_type|read_varint|decode_serial|get_serial_type_size|sqlite3GetVarint|sqlite3VdbeSerialGet|getVarint"),
    ("interpreter", r"vdbe::execute|dispatch_loop|\bop_[a-z_]+|Program>?::|ProgramState|sqlite3VdbeExec|sqlite3VdbeHalt|sqlite3VdbeReset|sqlite3VdbeFreeCursor"),
    ("values and rows", r"Register>?::|vdbe::Register|Value>?::|types::Text|replace_with_bytes|get_values|start_text_register|start_blob_register|Row>?::|sqlite3VdbeMem|sqlite3_column|columnMem|sqlite3ValueText|sqlite3_value_"),
    ("statement and binding", r"Statement>?::|bind_at|::reset|::prepare|sqlite3_step|sqlite3_reset|sqlite3_bind|sqlite3_clear_bindings|sqlite3VdbeMakeReady"),
    ("store: reader checkout", r"amphora::store|amphora::groups|amphora::meter|read_committed|Readers|give_back|amphora::db::"),
    ("locks", r"parking_lot|RawMutex|RawRwLock|RwLock|Mutex|futex|lock_api|SpinLock|fast_lock|pthread_mutex|Condvar"),
    ("allocator", r"malloc|free|__rust_alloc|__rust_dealloc|__rust_realloc|__rust_no_alloc|alloc::|_int_malloc|_int_free|cfree|sqlite3Malloc|sqlite3_free|sqlite3DbFree|sqlite3DbMalloc"),
    ("bench: timing and harness", r"amphora_bench::|suite::|Hist::|rusqlite::|params_from_iter|black_box|clock_gettime|Instant::now|foldhash|hashbrown"),
]
COMPILED = [(name, re.compile(p)) for name, p in PARTS]


def part_of(sym, dso=""):
    if sym in ("[unknown]", ""):
        # perf found no symbol: libc's internals are stripped on this system (memcpy's variants,
        # lock waits); the rest lie outside any mapped object.
        return "libc internals (no symbols)" if "libc.so" in dso else "unresolved addresses"
    for name, rx in COMPILED:
        if rx.search(sym):
            return name
    return "other"


def samples(path):
    """(leaf symbol, period) for each sample of a perf.data, its callchain's leaf first."""
    out = subprocess.run(["perf", "script", "-i", path, "-F", "period,ip,sym,dso", "--no-inline"], capture_output=True, text=True, errors="replace")
    period, leaf = None, None
    for line in out.stdout.splitlines():
        if not line.strip():
            if period is not None and leaf is not None:
                yield leaf, period
            period, leaf = None, None
            continue
        if not line.startswith(("\t", " ")) or re.match(r"^\s*\d+\s*$", line):
            m = re.match(r"^\s*(\d+)\s*$", line)
            if m:
                period = int(m.group(1))
            continue
        if leaf is None:
            m = re.match(r"^\s*[0-9a-f]+\s+(.*?)\s+\((.*)\)$", line)
            leaf = (m.group(1), m.group(2)) if m else (line.strip(), "")
    if period is not None and leaf is not None:
        yield leaf, period


def shares(path, top):
    """Each part's share of a recording's sampled cycles, the samples' total, and its `top` leaf
    functions' shares as (function, part, share)."""
    tot, funcs = collections.Counter(), collections.Counter()
    for (sym, dso), period in samples(path):
        part = part_of(sym, dso)
        tot[part] += period
        funcs[(sym, part)] += period
    s = sum(tot.values()) or 1
    return {k: v / s for k, v in tot.items()}, sum(tot.values()), [(f, p, v / s) for (f, p), v in funcs.most_common(top)]


def cpu(json_path):
    try:
        r = json.loads(open(json_path).read().strip().splitlines()[-1])
        x = r["results"][0]
        return x.get("cpu_user_s"), x.get("cpu_sys_s"), x.get("ops_per_s")
    except (OSError, ValueError, IndexError, KeyError):
        return None, None, None


def main():
    d = sys.argv[1]
    top = int(sys.argv[sys.argv.index("--top") + 1]) if "--top" in sys.argv else 0
    runs = collections.defaultdict(dict)
    for f in sorted(os.listdir(d)):
        m = re.match(r"(amphora|sqlite)-(.+)-(1|32)\.data$", f)
        if m:
            runs[(m.group(2), int(m.group(3)))][m.group(1)] = os.path.join(d, f)
    names = [n for n, _ in PARTS] + ["libc internals (no symbols)", "unresolved addresses", "other"]
    for (stmt, clients), by in sorted(runs.items(), key=lambda kv: (kv[0][1], kv[0][0])):
        cols = {}
        for t in ("amphora", "sqlite"):
            if t in by:
                sh, n, funcs = shares(by[t], top)
                u, s, ops = cpu(by[t].replace(".data", "-record.json"))
                cols[t] = (sh, n, u, s, ops, funcs)
        print(f"### {stmt} at {clients} client{'s' if clients > 1 else ''}")
        print()
        hdr = "| part | " + " | ".join(cols) + " |"
        print(hdr)
        print("|---|" + "---:|" * len(cols))
        for n in names:
            if any(cols[t][0].get(n, 0) >= 0.005 for t in cols):
                print(f"| {n} | " + " | ".join(f"{100 * cols[t][0].get(n, 0):.1f}%" for t in cols) + " |")
        print("| (user samples) | " + " | ".join(f"{cols[t][1]:,}" for t in cols) + " |")
        print("| kernel share of CPU (cpu_sys_s / (user + sys)) | " + " | ".join(f"{100 * cols[t][3] / (cols[t][2] + cols[t][3]):.0f}%" if cols[t][2] else "-" for t in cols) + " |")
        print("| statements a second in the profiled run | " + " | ".join(f"{cols[t][4]:,.0f}" if cols[t][4] else "-" for t in cols) + " |")
        print()
        for t in cols if top else ():
            print(f"| {t}: top leaf functions | part | share |")
            print("|---|---|---:|")
            for f, p, v in cols[t][5]:
                name = re.sub(r"<([^<>]+) as [^<>]+>::", r"<\1>::", f)[:110].replace("|", "\\|")
                print(f"| `{name}` | {p} | {100 * v:.1f}% |")
            print()


main()
