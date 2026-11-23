#!/usr/bin/env bash
# Unit 3 of docs/design/postgres.md section 11: PostgreSQL 19's raw_parser taken out of REL_19_0 by
# libpg_query's extraction, with none of libpg_query's patches (its 01, 09 and 16 make it accept
# `SELECT $1abc` and `NOTIFY ch, $1`, which 19 refuses 42601).
#
# Linux, with docker and network; nothing installed on the host. libpg_query's script is written
# for macOS: it runs here in ruby:3.3 with Debian's libclang, ffi-clang 0.14.0 (0.15 and later
# take no arguments to Index.new, which the script passes two) and an `xcrun` that answers `/` so
# its SDK include path is /usr/include.
#
# usage: scripts/postgres-oracle-extract.sh <work dir under ~/.cache>
set -euo pipefail
W=${1:?usage: postgres-oracle-extract.sh <work dir>}
mkdir -p "$W"
cd "$W"
[ -d postgres/.git ] || git clone -q --depth 1 --branch REL_19_0 https://github.com/postgres/postgres.git postgres
[ -d libpg_query/.git ] || git clone -q https://github.com/pganalyze/libpg_query.git libpg_query
git -C libpg_query checkout -q 7663e07
# The one libpg_query patch taken: its 12 drops an ACL check from functioncmds.c, which the
# extraction's walk reaches through PL/pgSQL's compiler (interpret_function_parameter_list) and
# stops at (`ERROR: Hit blocklist entry object_aclcheck`). raw_parser never runs that file, so no
# parse verdict or tree changes; the parser patches (01, 03, 04, 09, 14, 15, 16) stay out.
if ! patch -d postgres -p1 --dry-run -R -s < libpg_query/patches/12_remove_acl_check.patch >/dev/null 2>&1; then
  patch -d postgres -p1 -s < libpg_query/patches/12_remove_acl_check.patch
fi
docker run --rm -e HU="$(id -u)" -e HG="$(id -g)" -v "$W":/w ruby:3.3 bash -ec '
  trap "chown -R $HU:$HG /w" EXIT
  apt-get -qq update >/dev/null
  apt-get -qq install -y bison flex libclang-dev >/dev/null 2>&1
  export LIBCLANG=$(ls /usr/lib/llvm-*/lib/libclang.so* | head -1)
  gem install ffi-clang -v 0.14.0 >/dev/null 2>&1
  printf "#!/bin/sh\necho /\n" > /usr/local/bin/xcrun
  chmod +x /usr/local/bin/xcrun
  cd /w/postgres
  [ -f config.status ] || ./configure -q --without-icu --without-readline --without-zlib >/dev/null
  make -s -C src/pl/plpgsql/src pl_gram.h plerrcodes.h pl_reserved_kwlist_d.h pl_unreserved_kwlist_d.h
  make -s -C src/port pg_config_paths.h
  make -s -C src/backend generated-headers
  make -s -C src/backend parser-recursive
  make -s -C src/common kwlist_d.h
  # libpg_query'"'"'s own build steps after configure (its Makefile), none of them a parser change.
  if ! grep -q "undef StaticAssertDecl" src/include/c.h; then
    echo "#undef StaticAssertDecl" >> src/include/c.h
    echo "#define StaticAssertDecl(condition, errmessage)" >> src/include/c.h
    cat /w/libpg_query/scripts/pg_config_overrides.h >> src/include/pg_config.h
  fi
  rm -rf "${OUT:=/w/out}"
  mkdir -p "$OUT/include"
  ruby /w/libpg_query/scripts/extract_source.rb /w/postgres/ "$OUT/" > /w/extract.log 2>&1
  echo "extracted $(find "$OUT" -type f | wc -l) files into $OUT"'
