#!/usr/bin/env python3
"""The planner audit's benchmark map (docs/research/planner.md, "The audit"): for each query of JOB,
TPC-H, TPC-DS, CH-benCHmark and ClickBench, the shapes the audit's rows turn on, read from the
query's text: the most tables one FROM clause joins, the join kinds, and the subqueries.

python3 bench/planner/audit/features.py --job DIR --tpch DIR --tpcds DIR --ch DIR --clickbench FILE

A subquery counts as correlated when it names a table alias or table of the block around it that
it doesn't define itself, or (the unprefixed columns of TPC-H, TPC-DS and CH-benCHmark) a column prefix only the outer
block's tables carry. The text is scanned, not parsed: the map is checked by hand where the audit
cites it.
"""

import argparse
import json
import pathlib
import re
import sys

KEYWORDS = {
    "select", "from", "where", "group", "order", "having", "limit", "union", "intersect", "except",
    "join", "inner", "left", "right", "full", "outer", "cross", "natural", "on", "using", "as",
    "and", "or", "not", "in", "exists", "lateral", "with", "by", "window", "qualify", "offset",
    "fetch", "returning", "values",
}
ENDS = {"where", "group", "order", "having", "limit", "union", "intersect", "except", "window", "offset", "fetch"}


def tokens(sql):
    sql = re.sub(r"--[^\n]*", " ", sql)
    sql = re.sub(r"/\*.*?\*/", " ", sql, flags=re.S)
    sql = re.sub(r"'(?:[^']|'')*'", " 'lit' ", sql)
    return re.findall(r"[A-Za-z_][A-Za-z_0-9$#.]*|\(|\)|,|;|[^\sA-Za-z_(),;]+", sql)


def blocks(toks):
    """Each SELECT block: its depth's tokens, the tokens of its subqueries apart."""
    out = []
    stack = []  # (depth, block)
    depth = 0
    for i, t in enumerate(toks):
        low = t.lower()
        if t == "(":
            depth += 1
            continue
        if t == ")":
            while stack and stack[-1][0] >= depth:
                stack.pop()
            depth -= 1
            continue
        if low == "select" and (not stack or stack[-1][0] < depth or True):
            # A SELECT at a depth starts a block there (a UNION's next SELECT replaces the last).
            while stack and stack[-1][0] >= depth:
                stack.pop()
            parent = stack[-1][1] if stack else None
            b = {"depth": depth, "toks": [], "parent": parent, "start": i, "kind": subquery_kind(toks, i)}
            out.append(b)
            stack.append((depth, b))
            continue
        if stack:
            stack[-1][1]["toks"].append((t, depth))
    return out


def subquery_kind(toks, i):
    """What the subquery starting at toks[i] (SELECT) is: from the tokens before its parenthesis."""
    j = i - 1
    while j >= 0 and toks[j] == "(":
        j -= 1
    if j < 0 or toks[i - 1] != "(":
        return "top"
    prev = toks[j].lower()
    prev2 = toks[j - 1].lower() if j >= 1 else ""
    if prev == "exists":
        return "not exists" if prev2 == "not" else "exists"
    if prev == "in":
        return "not in" if prev2 == "not" else "in"
    if prev in ("from", "join", ","):
        return "derived"
    if prev == "as":
        return "cte"
    return "scalar"


def from_tables(b):
    """The FROM clause's items at the block's own depth: (name, alias) pairs, and join kinds."""
    items, kinds = [], set()
    toks = [t for t, d in b["toks"] if d == b["depth"]]
    alltoks = b["toks"]
    in_from = False
    expect = False
    k = 0
    while k < len(alltoks):
        t, d = alltoks[k]
        low = t.lower()
        if d == b["depth"]:
            if low == "from":
                in_from, expect = True, True
            elif in_from and low in ENDS:
                in_from = False
            elif in_from and low in ("on", "using"):
                expect = False
            elif in_from and t == ",":
                expect = True
            elif in_from and low == "join":
                expect = True
                back = [x[0].lower() for x in alltoks[max(0, k - 3):k]]
                for kind in ("left", "right", "full", "cross"):
                    if kind in back:
                        kinds.add(kind)
                if not ({"left", "right", "full", "cross"} & set(back)):
                    kinds.add("inner")
            elif in_from and expect and low not in KEYWORDS and re.match(r"[A-Za-z_]", t):
                alias = None
                n = k + 1
                if n < len(alltoks) and alltoks[n][0].lower() == "as":
                    n += 1
                if n < len(alltoks) and alltoks[n][1] == d and re.match(r"[A-Za-z_]", alltoks[n][0]) and alltoks[n][0].lower() not in KEYWORDS:
                    alias = alltoks[n][0]
                items.append((t.split(".")[-1], alias))
                expect = False
        elif in_from and expect and d == b["depth"] + 1:
            # A derived table or a parenthesized join: one item.
            items.append(("(subquery)", None))
            expect = False
            while k < len(alltoks) and alltoks[k][1] > b["depth"]:
                k += 1
            if k < len(alltoks):
                t, d = alltoks[k]
                if t.lower() == "as" and k + 1 < len(alltoks):
                    items[-1] = ("(subquery)", alltoks[k + 1][0])
            continue
        k += 1
    return items, kinds


def names_in(b):
    return {t for t, d in b["toks"] if re.match(r"[A-Za-z_]", t)}


def analyse(sql, prefixes=None):
    toks = tokens(sql)
    bs = blocks(toks)
    info = {"max_tables": 0, "joins": set(), "subqueries": []}
    own = {}
    for b in bs:
        items, kinds = from_tables(b)
        b["items"] = items
        info["max_tables"] = max(info["max_tables"], len(items))
        info["joins"] |= kinds
        own[id(b)] = {a or n for n, a in items} | {n for n, a in items}
    for b in bs:
        if b["kind"] in ("top", "derived", "cte"):
            continue
        outer = set()
        p = b["parent"]
        while p is not None:
            outer |= own[id(p)]
            p = p["parent"]
        mine = own[id(b)]
        qualified = {t.split(".")[0] for t in names_in(b) if "." in t}
        correlated = bool((qualified & outer) - mine)
        if not correlated and prefixes:
            # Unprefixed columns (CH-benCHmark): a column whose prefix belongs to an outer table only.
            mine_prefixes = {prefixes.get(n.lower()) for n, _ in b["items"]}
            outer_prefixes = {prefixes.get(n.lower()) for n in outer} - mine_prefixes - {None}
            cols = {t.lower().split("_")[0] + "_" for t in names_in(b) if "_" in t and "." not in t}
            correlated = bool(cols & outer_prefixes)
        info["subqueries"].append(b["kind"] + (" correlated" if correlated else ""))
    info["joins"] = sorted(info["joins"])
    return info


def java_sql(path):
    text = path.read_text()
    body = text.split("new SQLStmt(", 1)[1].split(");", 1)[0]
    return "".join(re.findall(r'"((?:[^"\\]|\\.)*)"', body))


# CH-benCHmark's tables and their columns' prefixes (TPC-C's schema, CH's three added tables).
CH_PREFIXES = {
    "warehouse": "w_", "district": "d_", "customer": "c_", "history": "h_", "new_order": "no_",
    "oorder": "o_", "order_line": "ol_", "item": "i_", "stock": "s_", "supplier": "su_",
    "nation": "n_", "region": "r_",
}


# TPC-H's and TPC-DS's column prefixes, as their specifications name the columns.
TPCH_PREFIXES = {
    "part": "p_", "supplier": "s_", "partsupp": "ps_", "customer": "c_", "orders": "o_",
    "lineitem": "l_", "nation": "n_", "region": "r_",
}
TPCDS_PREFIXES = {
    "store_sales": "ss_", "store_returns": "sr_", "catalog_sales": "cs_", "catalog_returns": "cr_",
    "web_sales": "ws_", "web_returns": "wr_", "inventory": "inv_", "store": "s_", "call_center": "cc_",
    "catalog_page": "cp_", "web_site": "web_", "web_page": "wp_", "warehouse": "w_", "customer": "c_",
    "customer_address": "ca_", "customer_demographics": "cd_", "date_dim": "d_",
    "household_demographics": "hd_", "item": "i_", "income_band": "ib_", "promotion": "p_",
    "reason": "r_", "ship_mode": "sm_", "time_dim": "t_",
}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--job")
    ap.add_argument("--tpch")
    ap.add_argument("--tpcds")
    ap.add_argument("--ch")
    ap.add_argument("--clickbench")
    ap.add_argument("--json", action="store_true")
    a = ap.parse_args()
    rows = []
    if a.job:
        for p in sorted(pathlib.Path(a.job).glob("[0-9]*.sql"), key=lambda p: (int(re.match(r"\d+", p.stem).group()), p.stem)):
            rows.append(("JOB", p.stem, analyse(p.read_text())))
    if a.tpch:
        for p in sorted(pathlib.Path(a.tpch).glob("q*.sql")):
            rows.append(("TPC-H", "Q" + str(int(p.stem[1:])), analyse(p.read_text(), TPCH_PREFIXES)))
    if a.tpcds:
        for p in sorted(pathlib.Path(a.tpcds).glob("*.sql")):
            rows.append(("TPC-DS", "Q" + str(int(p.stem)), analyse(p.read_text(), TPCDS_PREFIXES)))
    if a.ch:
        for p in sorted(pathlib.Path(a.ch).glob("Q*.java"), key=lambda p: int(p.stem[1:])):
            rows.append(("CH-benCHmark", p.stem, analyse(java_sql(p), CH_PREFIXES)))
    if a.clickbench:
        for i, q in enumerate(x for x in pathlib.Path(a.clickbench).read_text().split(";\n") if x.strip()):
            rows.append(("ClickBench", "Q" + str(i), analyse(q)))
    if a.json:
        json.dump([{"bench": b, "query": q, **i} for b, q, i in rows], sys.stdout, indent=1)
        return
    for b, q, i in rows:
        subs = ", ".join(i["subqueries"]) or "-"
        print(f"{b}\t{q}\t{i['max_tables']}\t{','.join(i['joins']) or '-'}\t{subs}")


if __name__ == "__main__":
    main()
