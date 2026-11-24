#!/usr/bin/env python3
"""PostgreSQL 19's built-in catalog rows for docs/design/postgres.md section 9 (section 11's unit 7).

Runs PostgreSQL's own genbki.pl (vendored, REL_19_0) on the vendored catalog headers and .dat files,
as PostgreSQL's build does, and lifts the rows its postgres.bki inserts into pg_type, pg_namespace,
pg_class and pg_attribute, and what analysis reads of pg_type, pg_proc, pg_operator, pg_cast and
pg_range (section 11's unit 8), into crates/pg-catalog/src/data.rs. Nothing in that file is written by hand.

usage: scripts/postgres-catalog.py <work dir under ~/.cache>
"""

from __future__ import annotations

import hashlib
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PG = ROOT / "vendor/postgres"
OUT = ROOT / "crates/pg-catalog/src/data.rs"

# The catalogs served whole, in the order data.rs lists them.
SERVED = ["pg_namespace", "pg_type", "pg_class", "pg_attribute"]

# How each bki column type is carried. Types not named here must be _null_ in every lifted row.
KIND = {
    "oid": "Oid",
    "regproc": "Oid",
    "xid": "Xid",
    "name": "Name",
    "int2": "Int2",
    "int4": "Int4",
    "bool": "Bool",
    "char": "Char",
    "float4": "Float4",
    "text": "Text",
    "pg_node_tree": "Text",
}


def inputs() -> list[Path]:
    """Every file genbki.pl reads, sorted: the hash data.rs records is over these."""
    files = [p for p in (PG / "src/include/catalog").iterdir() if p.is_file()]
    files += [
        PG / "src/backend/catalog/genbki.pl",
        PG / "src/backend/catalog/Catalog.pm",
        PG / "src/include/access/transam.h",
        PG / "src/include/mb/pg_wchar.h",
        PG / "src/include/pg_config_manual.h",
    ]
    return sorted(files, key=lambda p: p.relative_to(PG).as_posix())


def inputs_sha256() -> str:
    h = hashlib.sha256()
    for p in inputs():
        h.update(p.relative_to(PG).as_posix().encode() + b"\0" + p.read_bytes() + b"\0")
    return h.hexdigest()


def symbols() -> dict[str, str]:
    """The tokens initdb replaces in postgres.bki (src/bin/initdb/initdb.c:1578-1586 at REL_19_0).

    NAMEDATALEN is pg_config_manual.h's; SIZEOF_POINTER and ALIGNOF_POINTER are a 64-bit target's,
    the only targets Amphora builds (CLAUDE.md section 12: x86_64 and aarch64)."""
    m = re.search(r"^#define NAMEDATALEN (\d+)$", (PG / "src/include/pg_config_manual.h").read_text(), re.M)
    if not m:
        sys.exit("NAMEDATALEN not found in pg_config_manual.h")
    return {"NAMEDATALEN": m.group(1), "SIZEOF_POINTER": "8", "ALIGNOF_POINTER": "d"}


def tokens(line: str) -> list[str | None]:
    """An insert's values: bare words, or single-quoted with '' for a quote (genbki.pl's quoting)."""
    out: list[str | None] = []
    i, n = 0, len(line)
    while i < n:
        c = line[i]
        if c == " ":
            i += 1
        elif c == "'":
            j, buf = i + 1, []
            while True:
                if j >= n:
                    sys.exit(f"unterminated quote: {line}")
                if line[j] == "'":
                    if j + 1 < n and line[j + 1] == "'":
                        buf.append("'")
                        j += 2
                        continue
                    break
                if line[j] == "\\":
                    sys.exit(f"backslash in a lifted value: {line}")
                buf.append(line[j])
                j += 1
            out.append("".join(buf))
            i = j + 1
        else:
            j = line.find(" ", i)
            j = n if j < 0 else j
            word = line[i:j]
            out.append(None if word == "_null_" else word)
            i = j
    return out


def read_bki(path: Path) -> dict[str, dict]:
    rels: dict[str, dict] = {}
    cur = None
    in_cols = False
    for raw in path.read_text().splitlines():
        line = raw.strip()
        if line.startswith("create "):
            parts = line.split()
            cur = {"oid": int(parts[2]), "columns": [], "rows": []}
            rels[parts[1]] = cur
        elif line == "(" and cur is not None:
            in_cols = True
        elif line == ")" and in_cols:
            in_cols = False
        elif in_cols and cur is not None:
            name, _, ty = line.rstrip(" ,").partition(" = ")
            cur["columns"].append((name, ty.split()[0]))
        elif line.startswith("insert ( ") and line.endswith(" )") and cur is not None:
            cur["rows"].append(tokens(line[len("insert ( ") : -len(" )")]))
        elif line.startswith("close "):
            cur = None
    return rels


def rust_str(s: str) -> str:
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'


def datum(kind: str, ty: str, value: str | None, syms: dict[str, str]) -> str:
    if value is None:
        return "Null"
    value = syms.get(value, value)
    if kind == "Oid":
        return "Oid(0)" if value == "-" else f"Oid({int(value)})"
    if kind == "Xid":
        return f"Xid({int(value)})"
    if kind in ("Int2", "Int4"):
        return f"{kind}({int(value)})"
    if kind == "Float4":
        return f"Float4({float(value)!r})"
    if kind == "Bool":
        if value not in ("t", "f"):
            sys.exit(f"bool {value!r}")
        return f"Bool({'true' if value == 't' else 'false'})"
    if kind == "Char":
        if value == "":
            # charin of an empty string is the zero byte (src/backend/utils/adt/char.c:charin).
            return "Char(0)"
        if len(value) != 1 or not value.isascii():
            sys.exit(f"char {value!r}")
        return f"Char({ord(value)})"
    if kind in ("Name", "Text"):
        return f"{kind}({rust_str(value)})"
    sys.exit(f"type {ty} holds {value!r}; data.rs carries it only as NULL")


def by_name(rel: dict) -> list[dict[str, str | None]]:
    """A catalog's rows as column name to value."""
    names = [c for c, _ in rel["columns"]]
    return [dict(zip(names, row)) for row in rel["rows"]]


def oid(value: str | None) -> int:
    """An oid or regproc value: `-` (and NULL) is InvalidOid."""
    return 0 if value in (None, "-") else int(value)


def flag(value: str | None) -> str:
    if value not in ("t", "f"):
        sys.exit(f"bool {value!r}")
    return "true" if value == "t" else "false"


def char(value: str | None) -> str:
    if value is None or len(value) != 1 or not value.isascii():
        sys.exit(f"char {value!r}")
    return f"b'{value}'"


def types_table(rel: dict) -> list[str]:
    """What analysis reads of pg_type, by OID (its syscache key TYPEOID)."""
    out = ["", "pub static TYPES: &[Type] = &["]
    for r in sorted(by_name(rel), key=lambda r: int(r["oid"] or 0)):
        out.append(
            f"    Type {{ oid: {oid(r['oid'])}, name: {rust_str(r['typname'] or '')}, namespace: {oid(r['typnamespace'])}, "
            f"kind: {char(r['typtype'])}, category: {char(r['typcategory'])}, preferred: {flag(r['typispreferred'])}, "
            f"relid: {oid(r['typrelid'])}, subscript: {oid(r['typsubscript'])}, elem: {oid(r['typelem'])}, "
            f"array: {oid(r['typarray'])}, base: {oid(r['typbasetype'])}, typtypmod: {int(r['typtypmod'] or -1)}, "
            f"input: {oid(r['typinput'])}, output: {oid(r['typoutput'])}, modin: {oid(r['typmodin'])}, collation: {oid(r['typcollation'])} }},"
        )
    out += ["];", "", "/// Each built-in type's OID by its name (arrays aside), as pg_type_d.h's symbols name them.", "pub mod type_oid {"]
    for r in sorted(by_name(rel), key=lambda r: int(r["oid"] or 0)):
        if not (r["typname"] or "_").startswith("_"):
            out.append(f"    pub const {(r['typname'] or '').upper()}: u32 = {oid(r['oid'])};")
    return out + ["}"]


def ranges(rel: dict) -> list[str]:
    """What analysis reads of pg_range, by range type."""
    out = ["", "pub static PG_RANGE: &[Range] = &["]
    for r in sorted(by_name(rel), key=lambda r: oid(r["rngtypid"])):
        out.append(f"    Range {{ oid: {oid(r['rngtypid'])}, subtype: {oid(r['rngsubtype'])}, multirange: {oid(r['rngmultitypid'])} }},")
    return out + ["];"]


def procs(rel: dict, types: dict[str, int]) -> list[str]:
    """What analysis reads of pg_proc, by OID, and the same rows' places in order of name."""
    rows = sorted(by_name(rel), key=lambda r: int(r["oid"] or 0))
    out = ["", "pub static PG_PROC: &[Proc] = &["]
    for r in rows:
        args = ", ".join(str(types.get(a, a)) for a in (r["proargtypes"] or "").split())
        out.append(
            f"    Proc {{ oid: {oid(r['oid'])}, name: {rust_str(r['proname'] or '')}, namespace: {oid(r['pronamespace'])}, "
            f"kind: {char(r['prokind'])}, strict: {flag(r['proisstrict'])}, retset: {flag(r['proretset'])}, "
            f"volatile: {char(r['provolatile'])}, variadic: {oid(r['provariadic'])}, defaults: {int(r['pronargdefaults'] or 0)}, "
            f"result: {oid(r['prorettype'])}, args: &[{args}], src: {rust_str(r['prosrc'] or '')} }},"
        )
    out += ["];", "", "pub static PG_PROC_BY_NAME: &[u16] = &["]
    order = sorted(range(len(rows)), key=lambda i: ((rows[i]["proname"] or "").encode(), i))
    for i in range(0, len(order), 16):
        out.append("    " + " ".join(f"{n}," for n in order[i : i + 16]))
    return out + ["];"]


def operators(rel: dict) -> list[str]:
    """What analysis reads of pg_operator, in order of name, kind, left and right operand."""
    key = lambda r: ((r["oprname"] or "").encode(), r["oprkind"], oid(r["oprleft"]), oid(r["oprright"]))
    out = ["", "pub static PG_OPERATOR: &[Operator] = &["]
    for r in sorted(by_name(rel), key=key):
        out.append(
            f"    Operator {{ oid: {oid(r['oid'])}, name: {rust_str(r['oprname'] or '')}, namespace: {oid(r['oprnamespace'])}, "
            f"kind: {char(r['oprkind'])}, left: {oid(r['oprleft'])}, right: {oid(r['oprright'])}, "
            f"result: {oid(r['oprresult'])}, code: {oid(r['oprcode'])} }},"
        )
    return out + ["];"]


def casts(rel: dict) -> list[str]:
    """pg_cast, in order of source and target (its syscache key CASTSOURCETARGET)."""
    out = ["", "pub static PG_CAST: &[Cast] = &["]
    for r in sorted(by_name(rel), key=lambda r: (oid(r["castsource"]), oid(r["casttarget"]))):
        out.append(
            f"    Cast {{ source: {oid(r['castsource'])}, target: {oid(r['casttarget'])}, func: {oid(r['castfunc'])}, "
            f"context: {char(r['castcontext'])}, method: {char(r['castmethod'])} }},"
        )
    return out + ["];"]


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    work = Path(sys.argv[1]).resolve()
    work.mkdir(parents=True, exist_ok=True)
    headers = sorted(str(p) for p in (PG / "src/include/catalog").glob("pg_*.h") if not p.name.endswith("_d.h"))
    subprocess.run(
        ["perl", str(PG / "src/backend/catalog/genbki.pl"), f"--include-path={PG / 'src/include'}/",
         "--set-version=19", f"--output={work}/", *headers],
        check=True, cwd=work,
    )
    rels = read_bki(work / "postgres.bki")
    syms = symbols()
    types = {r[1]: int(r[0]) for r in rels["pg_type"]["rows"]}
    out = [
        "//! PostgreSQL 19's built-in rows of pg_namespace, pg_type, pg_class and pg_attribute, and what analysis",
        "//! reads of pg_type, pg_proc, pg_operator, pg_cast and pg_range: PostgreSQL's own genbki.pl run on the vendored",
        "//! catalog headers and .dat files (vendor/postgres, REL_19_0), its postgres.bki lifted by",
        "//! scripts/postgres-catalog.py. Generated; do not edit.",
        "",
        "use crate::Datum::{USED};",
        "use crate::{Cast, Catalog, Column, Operator, Proc, Range, Type};",
        "",
        "/// sha256 over the files genbki.pl read (scripts/postgres-catalog.py `inputs`).",
        f"pub const INPUTS_SHA256: &str = \"{inputs_sha256()}\";",
    ]
    for rel in SERVED:
        r = rels[rel]
        kinds = [KIND.get(ty, "") for _, ty in r["columns"]]
        const = rel.upper()
        out += ["", f"pub static {const}: Catalog = Catalog {{", f"    oid: {r['oid']},", f"    name: \"{rel}\",", "    columns: &["]
        for name, ty in r["columns"]:
            if ty not in types:
                sys.exit(f"{rel}.{name}: type {ty} has no pg_type row")
            out.append(f"        Column {{ name: \"{name}\", type_oid: {types[ty]} }},")
        out += ["    ],", "    rows: &["]
        for row in r["rows"]:
            if len(row) != len(kinds):
                sys.exit(f"{rel}: {len(row)} values for {len(kinds)} columns")
            vals = ", ".join(datum(k, ty, v, syms) for k, (_, ty), v in zip(kinds, r["columns"], row))
            out.append(f"        &[{vals}],")
        out += ["    ],", "};"]
    out += types_table(rels["pg_type"]) + procs(rels["pg_proc"], types) + operators(rels["pg_operator"]) + casts(rels["pg_cast"]) + ranges(rels["pg_range"])
    text = "\n".join(out) + "\n"
    used = sorted(v for v in ("Bool", "Char", "Float4", "Int2", "Int4", "Name", "Null", "Oid", "Text", "Xid") if re.search(rf"\b{v}\b", text.split("{USED}", 1)[1]))
    OUT.write_text(text.replace("{USED}", "{" + ", ".join(used) + "}", 1))
    print(f"wrote {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
