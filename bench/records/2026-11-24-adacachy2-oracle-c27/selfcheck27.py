#!/usr/bin/env python3
"""Lane C27: does SQLite agree with itself on a finding? Each finding's queries on the oracles' SQLite 3.54.0: as it stands, with every optimization off
(SQLITE_TESTCTRL_OPTIMIZATIONS), and with the outermost ORDER BY left out (no LIMIT): a row set can't depend on either. Writes id -> verdicts.
usage: selfcheck27.py LIB REPLAY.json OUT.json"""
import ctypes, json, re, sys
L = ctypes.CDLL(sys.argv[1])
L.sqlite3_column_text.restype = ctypes.c_void_p; L.sqlite3_column_blob.restype = ctypes.c_void_p
L.sqlite3_column_int64.restype = ctypes.c_int64; L.sqlite3_column_double.restype = ctypes.c_double
L.sqlite3_errmsg.restype = ctypes.c_char_p
CB = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p)
cb = CB(lambda _: 1)

class Db:
    def __init__(self, off=False):
        self.p = ctypes.c_void_p()
        assert L.sqlite3_open(b":memory:", ctypes.byref(self.p)) == 0
        L.sqlite3_progress_handler(self.p, 8_000_000, cb, None)
        self.off = off
    def opt(self):
        if self.off: L.sqlite3_test_control(15, self.p, ctypes.c_int(-1))
    def run(self, sql):
        st = ctypes.c_void_p(); b = sql.encode()
        if L.sqlite3_prepare_v2(self.p, b, len(b), ctypes.byref(st), None) != 0: return ("error", L.sqlite3_errmsg(self.p).decode(errors="replace"))
        rows = []
        while True:
            rc = L.sqlite3_step(st)
            if rc == 100:
                r = []
                for i in range(L.sqlite3_column_count(st)):
                    t = L.sqlite3_column_type(st, i)
                    if t == 1: r.append(("i", L.sqlite3_column_int64(st, i)))
                    elif t == 2: r.append(("r", repr(L.sqlite3_column_double(st, i))))
                    elif t == 5: r.append(("n",))
                    else:
                        p = (L.sqlite3_column_text if t == 3 else L.sqlite3_column_blob)(st, i); n = L.sqlite3_column_bytes(st, i)
                        r.append(("t" if t == 3 else "b", ctypes.string_at(p, n).hex() if p else ""))
                rows.append(tuple(r)); 
                if len(rows) > 200000: break
                continue
            L.sqlite3_finalize(st)
            return ("rows", sorted(map(repr, rows))) if rc == 101 else ("error", L.sqlite3_errmsg(self.p).decode(errors="replace"))
        L.sqlite3_finalize(st); return ("rows", sorted(map(repr, rows)))
    def close(self): L.sqlite3_close_v2(self.p)

def top(q):
    """(position, word) of each top-level keyword of interest, outside strings, quotes and parentheses"""
    out = []; d = 0; i = 0; n = len(q)
    while i < n:
        c = q[i]
        if c == "'":
            i += 1
            while i < n:
                if q[i] == "'":
                    if i + 1 < n and q[i + 1] == "'": i += 2; continue
                    break
                i += 1
        elif c == "(": d += 1
        elif c == ")": d -= 1
        elif d == 0 and q[i:i + 8].upper() == "ORDER BY" and (i == 0 or not q[i - 1].isalnum()): out.append((i, "ORDER BY"))
        elif d == 0 and q[i:i + 5].upper() == "LIMIT" and (i == 0 or not q[i - 1].isalnum()): out.append((i, "LIMIT"))
        i += 1
    return out
def no_order(q):
    t = top(q); o = [p for p, w in t if w == "ORDER BY"]
    if not o or any(w == "LIMIT" for _, w in t): return None
    return q[:o[-1]].rstrip()

rs = json.load(open(sys.argv[2])); out = {}
for r in rs:
    if r["rc"] != 0 or not r["differs"] or r["kind"] not in ("Wrong",): continue
    stmts = [s.strip() for s in r["stmts"].split(";\n") if s.strip()]; stmts[-1:] = [stmts[-1].rstrip(";")] if stmts else []
    qs = [s.strip().rstrip(";") for s in r["queries"].split(";\n") if s.strip()]
    a, b = Db(), Db(True)
    for s in stmts: a.run(s); b.run(s)
    b.opt(); v = []
    for q in qs:
        x, y = a.run(q), b.run(q); w = {"opt": x == y}
        nq = no_order(q)
        if nq is not None and x[0] == "rows":
            z = a.run(nq); w["order"] = x == z
            w["order_sql"] = nq if x != z else ""
        v.append(w)
    a.close(); b.close()
    out[r["id"]] = v
json.dump(out, open(sys.argv[3], "w"), indent=0)
bad_o = [k for k, v in out.items() if any(w.get("order") is False for w in v)]; bad_p = [k for k, v in out.items() if any(not w["opt"] for w in v)]
print("checked", len(out), "SQLite's rows change without its ORDER BY:", len(bad_o), "with its optimizations off:", len(bad_p))
