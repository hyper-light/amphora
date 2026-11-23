#!/usr/bin/env bash
# The release binaries native mode runs (lib.sh's MODE=native; CP-T1.9 to CP-T1.11, G13): each
# system's own published build for this machine, unpacked into DIR/<system>-<version>, the folder
# AMPHORA_BENCH_SCALE_BIN names. The versions are arguments: nothing is picked here. Each binary's
# SHA-256 is written beside it (SHA256SUMS), so a record can say exactly what ran.
#
# usage: bench/servers/scale/fetch.sh DIR cockroach VERSION   (binaries.cockroachdb.com, e.g. v25.4.0)
#        bench/servers/scale/fetch.sh DIR tidb VERSION        (PingCAP's TiUP mirror: pd, tikv, tidb, ctl)
set -euo pipefail
DIR=${1:?usage: fetch.sh DIR cockroach|tidb VERSION}
SYS=${2:?usage: fetch.sh DIR cockroach|tidb VERSION}
VERSION=${3:?usage: fetch.sh DIR cockroach|tidb VERSION}
case "$(uname -m)" in x86_64) ARCH=amd64 ;; aarch64 | arm64) ARCH=arm64 ;; *) echo "no release for $(uname -m)" >&2; exit 2 ;; esac
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
OUT="$DIR/$SYS-$VERSION"
mkdir -p "$OUT"
case "$SYS" in
  cockroach)
    curl -sfL "https://binaries.cockroachdb.com/cockroach-$VERSION.$OS-$ARCH.tgz" | tar -xz -C "$OUT" --strip-components 1
    ;;
  tidb)
    for c in pd tikv tidb ctl; do
      curl -sfL "https://tiup-mirrors.pingcap.com/$c-$VERSION-$OS-$ARCH.tar.gz" | tar -xz -C "$OUT"
    done
    ;;
  *) echo "fetch.sh: cockroach or tidb, not $SYS" >&2; exit 2 ;;
esac
(cd "$OUT" && find . -maxdepth 1 -type f -perm -u+x -exec sha256sum {} + >SHA256SUMS)
echo "$OUT"
