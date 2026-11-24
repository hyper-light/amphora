//! T-PG.7.1: the built-in catalog rows are PostgreSQL 19's. data.rs records the sha256 of the files
//! genbki.pl read; the pg_type rows are checked field by field against the vendored pg_type.dat,
//! read here independently of the generator.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::disallowed_macros
)]

use amphora_pg_catalog::Datum;
use amphora_pg_catalog::data::{INPUTS_SHA256, PG_TYPE};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn pg() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/postgres")
}

#[test]
fn t_pg_7_1_data_is_generated_from_the_vendored_catalog() {
    let pg = pg();
    let mut files: Vec<PathBuf> = std::fs::read_dir(pg.join("src/include/catalog"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .collect();
    for f in [
        "src/backend/catalog/genbki.pl",
        "src/backend/catalog/Catalog.pm",
        "src/include/access/transam.h",
        "src/include/mb/pg_wchar.h",
        "src/include/pg_config_manual.h",
    ] {
        files.push(pg.join(f));
    }
    let rel = |p: &PathBuf| {
        p.strip_prefix(&pg)
            .unwrap()
            .to_str()
            .unwrap()
            .replace('\\', "/")
    };
    files.sort_by_key(rel);
    let mut h = Sha256::new();
    for f in &files {
        h.update(rel(f).as_bytes());
        h.update(b"\0");
        h.update(std::fs::read(f).unwrap());
        h.update(b"\0");
    }
    assert_eq!(
        format!("{:x}", h.finalize()),
        INPUTS_SHA256,
        "data.rs was generated from other catalog files: regenerate it with scripts/postgres-catalog.py"
    );
}

/// The `{ key => 'value', ... }` entries of a .dat file (Perl data: single-quoted values with `\'`
/// and `\\` escapes; `#` comments).
fn read_dat(text: &str) -> Vec<BTreeMap<String, String>> {
    let body: String = text
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    let b = body.as_bytes();
    let (mut out, mut i) = (Vec::new(), 0);
    while let Some(open) = body[i..].find('{') {
        i += open + 1;
        let mut entry = BTreeMap::new();
        loop {
            while b[i] == b' ' || b[i] == b'\n' || b[i] == b',' {
                i += 1;
            }
            if b[i] == b'}' {
                i += 1;
                break;
            }
            let key_end = i + body[i..].find(' ').unwrap();
            let key = body[i..key_end].to_string();
            i = key_end + body[key_end..].find('\'').unwrap() + 1;
            let mut value = String::new();
            loop {
                match b[i] {
                    b'\\' => {
                        value.push(b[i + 1] as char);
                        i += 2;
                    }
                    b'\'' => {
                        i += 1;
                        break;
                    }
                    _ => {
                        let c = body[i..].chars().next().unwrap();
                        value.push(c);
                        i += c.len_utf8();
                    }
                }
            }
            entry.insert(key, value);
        }
        out.push(entry);
    }
    out
}

#[test]
fn t_pg_7_1_builtin_pg_type_rows_are_19s() {
    let dat =
        read_dat(&std::fs::read_to_string(pg().join("src/include/catalog/pg_type.dat")).unwrap());
    let col = |name: &str| PG_TYPE.column(name).unwrap();
    let by_oid: BTreeMap<u32, &[Datum]> = PG_TYPE
        .rows
        .iter()
        .map(|row| match row[col("oid")] {
            Datum::Oid(oid) => (oid, *row),
            other => panic!("oid {other:?}"),
        })
        .collect();
    assert_eq!(by_oid.len(), PG_TYPE.rows.len(), "OIDs are unique");
    // initdb's replacements (src/bin/initdb/initdb.c:1578-1586) on a 64-bit target.
    let symbol = |v: &str| match v {
        "NAMEDATALEN" => "64".to_string(),
        "SIZEOF_POINTER" => "8".to_string(),
        "ALIGNOF_POINTER" => "d".to_string(),
        v => v.to_string(),
    };
    let mut arrays = 0;
    for entry in &dat {
        let oid: u32 = entry["oid"].parse().unwrap();
        let row = by_oid
            .get(&oid)
            .unwrap_or_else(|| panic!("pg_type.dat's {oid} is missing"));
        assert_eq!(row[col("typname")], Datum::Name(&entry["typname"]), "{oid}");
        for (key, value) in entry {
            let value = symbol(value);
            let want = match key.as_str() {
                "typlen" => Datum::Int2(value.parse().unwrap()),
                "typbyval" | "typispreferred" => Datum::Bool(value == "t"),
                "typtype" | "typcategory" | "typdelim" | "typalign" | "typstorage" => {
                    Datum::Char(value.as_bytes()[0])
                }
                "typtypmod" | "typndims" => Datum::Int4(value.parse().unwrap()),
                _ => continue,
            };
            assert_eq!(row[col(key)], want, "{} {key}", entry["typname"]);
        }
        if let Some(array) = entry.get("array_type_oid") {
            // genbki.pl makes each array type from its element's array_type_oid (Catalog.pm
            // GenerateArrayTypes): named with a leading underscore, typelem the element.
            arrays += 1;
            let array: u32 = array.parse().unwrap();
            let arow = by_oid[&array];
            assert_eq!(
                arow[col("typname")],
                Datum::Name(&format!("_{}", entry["typname"]))
            );
            assert_eq!(arow[col("typelem")], Datum::Oid(oid));
            assert_eq!(arow[col("typcategory")], Datum::Char(b'A'));
            assert_eq!(row[col("typarray")], Datum::Oid(array));
        }
    }
    assert_eq!(
        PG_TYPE.rows.len(),
        dat.len() + arrays,
        "pg_type is pg_type.dat's rows and their array types"
    );
    assert!(
        dat.len() > 100 && arrays > 50,
        "non-vacuous: {} rows, {arrays} arrays",
        dat.len()
    );
}
