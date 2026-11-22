#!/usr/bin/env bash
# Fetches every competitor's release binaries into AMPHORA_REPL_BIN (default ~/.cache/amphora-repl-bin)
# on this host, for the native clusters beside this script (CP-T1.8; Ada's fair-comparison rule: every
# system on one host, natively). Run it once per host, from a checkout; it's safe to run again (what's
# there is kept; delete the folder to fetch again).
#   rqlite   rqlited from rqlite/rqlite's GitHub release
#   libsql   sqld from tursodatabase/libsql's newest libsql-server-v* release
#   litefs   litefs from superfly/litefs's release, and consul from releases.hashicorp.com (its lease)
#   dqlite   no release binary exists: the bench's shim (../shim/main.go) is built with Go (go.dev's
#            release, fetched here) against libdqlite: the host's (pkg-config dqlite) if it has one,
#            else canonical/dqlite's release source built here with its own raft (needs a C
#            compiler, autotools, libuv and sqlite3 headers). go-dqlite goes with the libdqlite it
#            builds against, as each states: libdqlite 1.18 and later drop
#            dqlite_node_enable_disk_mode, which go-dqlite v1 and v2 call (internal/bindings/server.go),
#            and go-dqlite v3 refuses a libdqlite older than 1.18 (its bindings check
#            DQLITE_VERSION_NUMBER < 11800). So libdqlite 1.18+ takes go-dqlite v3's newest release
#            (the shim's import moved to v3's module path), and an older one v1.22.0, as
#            ../shim/Dockerfile pairs with Ubuntu 24.04's libdqlite 1.16.
# Versions: the newest release of each at the time this runs, unless pinned in the environment
# (RQLITE_TAG, LIBSQL_TAG, LITEFS_TAG, CONSUL_VERSION, GO_VERSION, DQLITE_TAG, GO_DQLITE: the shim's
# go-dqlite, by default as above). The versions fetched go to $BIN/versions.txt, and each cluster's
# `about` puts its binary's own version line in every record.
# Exit: 0 when every system is ready; 1 when one isn't (each says why; the others are still set up,
# and the harness records the missing one's typed refusal).
set -uo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
BIN=${AMPHORA_REPL_BIN:-$HOME/.cache/amphora-repl-bin}
WORK="$BIN/.work"
mkdir -p "$BIN" "$WORK"
case "$(uname -m)" in
  x86_64 | amd64) ARCH=amd64 RUST_ARCH=x86_64 ;;
  aarch64 | arm64) ARCH=arm64 RUST_ARCH=aarch64 ;;
  *) echo "no release builds named for $(uname -m)" >&2; exit 1 ;;
esac
[[ "$(uname -s)" == Linux ]] || { echo "the native clusters are Linux's (pidfds, FUSE); this is $(uname -s)" >&2; exit 1; }

# A release's tag and the URL of its asset whose name matches ERE, from GitHub's API:
# release REPO TAG_PREFIX ERE [TAG] -> "tag url"
release() {
  local repo=$1 prefix=$2 ere=$3 tag=${4:-}
  local url="https://api.github.com/repos/$repo/releases?per_page=100"
  [[ -n "$tag" ]] && url="https://api.github.com/repos/$repo/releases/tags/$tag"
  curl -fsSL "$url" | python3 -I -c '
import json, re, sys
prefix, ere = sys.argv[1], re.compile(sys.argv[2])
data = json.load(sys.stdin)
for r in (data if isinstance(data, list) else [data]):
    if r.get("draft") or r.get("prerelease") or not r["tag_name"].startswith(prefix):
        continue
    for a in r.get("assets", []):
        if ere.search(a["name"]):
            print(r["tag_name"], a["browser_download_url"]); sys.exit(0)
sys.exit(1)' "$prefix" "$ere"
}

# Downloads URL and unpacks it into a new, empty folder of WORK, printing that folder.
fetch() {
  local url=$1 d
  d=$(mktemp -d "$WORK/x.XXXXXX")
  curl -fsSL -o "$d/archive" "$url" || return 1
  case "$url" in
    *.zip) python3 -I -c 'import sys, zipfile; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])' "$d/archive" "$d/out" ;;
    *) mkdir -p "$d/out" && tar -xf "$d/archive" -C "$d/out" ;;
  esac || return 1
  echo "$d/out"
}

# Puts the file named NAME under folder D at $BIN/NAME, executable.
install_from() {
  local d=$1 name=$2 f
  f=$(find "$d" -type f -name "$name" | head -n 1)
  [[ -n "$f" ]] || { echo "no $name in the release" >&2; return 1; }
  install -m 0755 "$f" "$BIN/$name"
}

note() { echo "$1 $2" >>"$BIN/versions.txt"; echo "$1: $2" >&2; }
failed=0

if [[ ! -x "$BIN/rqlited" ]]; then
  if read -r tag url < <(release rqlite/rqlite v "linux-$ARCH\\.tar\\.gz\$" "${RQLITE_TAG:-}") && d=$(fetch "$url") && install_from "$d" rqlited; then
    note rqlite "$tag"
  else
    echo "rqlite: not fetched" >&2; failed=1
  fi
fi

if [[ ! -x "$BIN/sqld" ]]; then
  if read -r tag url < <(release tursodatabase/libsql libsql-server-v "$RUST_ARCH-unknown-linux-gnu\\.tar\\.(xz|gz)\$" "${LIBSQL_TAG:-}") && d=$(fetch "$url") && install_from "$d" sqld; then
    note libsql "$tag"
  else
    echo "libsql: not fetched" >&2; failed=1
  fi
fi

if [[ ! -x "$BIN/litefs" ]]; then
  if read -r tag url < <(release superfly/litefs v "linux-$ARCH\\.tar\\.gz\$" "${LITEFS_TAG:-}") && d=$(fetch "$url") && install_from "$d" litefs; then
    note litefs "$tag"
  else
    echo "litefs: not fetched" >&2; failed=1
  fi
fi

if [[ ! -x "$BIN/consul" ]]; then
  v=${CONSUL_VERSION:-$(curl -fsSL https://checkpoint-api.hashicorp.com/v1/check/consul | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["current_version"])')}
  if [[ -n "$v" ]] && d=$(fetch "https://releases.hashicorp.com/consul/$v/consul_${v}_linux_$ARCH.zip") && install_from "$d" consul; then
    note consul "$v"
  else
    echo "consul: not fetched" >&2; failed=1
  fi
fi

# Go, for the shim: go.dev's release under $BIN/go.
if [[ ! -x "$BIN/go/bin/go" ]]; then
  v=${GO_VERSION:-$(curl -fsSL 'https://go.dev/VERSION?m=text' | head -n 1)}
  if [[ -n "$v" ]] && d=$(fetch "https://go.dev/dl/$v.linux-$ARCH.tar.gz") && [[ -x "$d/go/bin/go" ]]; then
    rm -rf "$BIN/go" && mv "$d/go" "$BIN/go" && note go "$v"
  else
    echo "go: not fetched" >&2; failed=1
  fi
fi

# libdqlite: the host's, else built from its release source into $BIN (lib, include).
if pkg-config --exists dqlite 2>/dev/null; then
  DQ_PC=""
  echo "dqlite $(pkg-config --modversion dqlite) (the host's)" >"$BIN/dqlite.version"
  DQ_VERSION=$(pkg-config --modversion dqlite)
elif [[ -f "$BIN/lib/pkgconfig/dqlite.pc" ]]; then
  DQ_PC="$BIN/lib/pkgconfig"
  DQ_VERSION=$(PKG_CONFIG_PATH="$DQ_PC" pkg-config --modversion dqlite)
else
  DQ_PC="$BIN/lib/pkgconfig"
  if read -r tag url < <(curl -fsSL "https://api.github.com/repos/canonical/dqlite/releases${DQLITE_TAG:+/tags/$DQLITE_TAG}" | python3 -I -c '
import json, sys
d = json.load(sys.stdin)
for r in (d if isinstance(d, list) else [d]):
    if not r.get("draft") and not r.get("prerelease"):
        print(r["tag_name"], r["tarball_url"]); break') \
    && d=$(mktemp -d "$WORK/dq.XXXXXX") && curl -fsSL -o "$d/src.tar.gz" "$url" && mkdir -p "$d/src" && tar -xzf "$d/src.tar.gz" -C "$d/src" --strip-components 1 \
    && (cd "$d/src" && autoreconf -i >/dev/null && ./configure --prefix="$BIN" --enable-build-raft >/dev/null && make -j"$(nproc)" >/dev/null && make install >/dev/null); then
    echo "dqlite $tag (built here)" >"$BIN/dqlite.version"
    DQ_VERSION=$(PKG_CONFIG_PATH="$DQ_PC" pkg-config --modversion dqlite)
    note dqlite "$tag"
  else
    echo "dqlite: libdqlite neither on this host nor built (needs gcc, autotools, libuv and sqlite3 headers)" >&2; failed=1
  fi
fi

if [[ ! -x "$BIN/shim" && -x "$BIN/go/bin/go" && -n "${DQ_VERSION:-}" ]]; then
  s=$(mktemp -d "$WORK/shim.XXXXXX")
  cp "$HERE/../shim/main.go" "$s/"
  # go-dqlite for this libdqlite (the header's comment says why): v3's module path is .../v3.
  IFS=. read -r dq_major dq_minor _ <<<"$DQ_VERSION"
  if ((dq_major > 1 || (dq_major == 1 && dq_minor >= 18))); then
    gdq_mod=github.com/canonical/go-dqlite/v3
    gdq=${GO_DQLITE:-$(curl -fsSL 'https://api.github.com/repos/canonical/go-dqlite/releases?per_page=100' | python3 -I -c '
import json, sys
print(next((r["tag_name"] for r in json.load(sys.stdin) if not r["draft"] and not r["prerelease"] and r["tag_name"].startswith("v3.")), ""))')}
    sed -i 's|"github.com/canonical/go-dqlite/app"|"github.com/canonical/go-dqlite/v3/app"|' "$s/main.go"
  else
    gdq_mod=github.com/canonical/go-dqlite gdq=${GO_DQLITE:-v1.22.0}
  fi
  if [[ -n "$gdq" ]] && (cd "$s" && export PATH="$BIN/go/bin:$PATH" GOPATH="$WORK/gopath" GOCACHE="$WORK/gocache" PKG_CONFIG_PATH="${DQ_PC}${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}" \
      && go mod init shim >/dev/null 2>&1 && go get "$gdq_mod@$gdq" github.com/mattn/go-sqlite3 >&2 && go mod tidy >&2 \
      && CGO_LDFLAGS_ALLOW="-Wl,-z,now" go build -tags libsqlite3 -o "$BIN/shim" .); then
    note go-dqlite "$gdq (libdqlite $DQ_VERSION)"
  else
    echo "shim: not built (go-dqlite against libdqlite; see above)" >&2; failed=1
  fi
fi
rm -rf "$WORK"/x.* "$WORK"/dq.* "$WORK"/shim.*
for b in rqlited sqld litefs consul shim; do [[ -x "$BIN/$b" ]] && echo "ready: $b" >&2 || echo "missing: $b" >&2; done
exit $failed
