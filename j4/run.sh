#!/bin/bash
# J4 integrator: G's t_qt_6_30 under one shared calibration (RULES 5e), then I's reruns.
set -u
export CARGO_HOME=$HOME/.cache/cargo-home-perf-j4
mkdir -p "$CARGO_HOME"
for d in registry git bin; do [ -e "$CARGO_HOME/$d" ] || ln -s "$HOME/.cargo/$d" "$CARGO_HOME/$d"; done
export PATH=$HOME/.cargo/bin:$PATH CARGO_NET_OFFLINE=false CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=8
export CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/integrate-j4
MAIN=${MAIN:?} G=${G:?} I=${I:?} PHASE=${PHASE:-all}
SRC=$HOME/.cache/amphora-perf/j4-remote LOGS=$HOME/.cache/amphora-perf/j4-logs BIN=$HOME/.cache/amphora-perf-bin/j4
XDG=$HOME/.cache/amphora-perf/j4/xdg-g630
mkdir -p "$LOGS" "$BIN" "$XDG"
[ -d "$SRC/.git" ] || git clone -q --reference $HOME/.cache/amphora-perf/j2-remote https://github.com/hyper-light/amphora.git "$SRC" || exit 2
cd "$SRC" || exit 2
git fetch -q origin '+refs/heads/*:refs/remotes/origin/*' || exit 2
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
build() { # label sha pkg...
  local label=$1 sha=$2; shift 2
  git checkout -q --detach "$sha" || exit 3
  local args=() p; for p in "$@"; do args+=(-p "$p"); done
  echo "== build $label $sha $* $(date -u +%H:%MZ) $(state)"
  nice -n 10 cargo test "${args[@]}" --lib --locked --no-run --message-format=json 2>"$LOGS/build-$label.err" >"$LOGS/build-$label.json" || { tail -20 "$LOGS/build-$label.err"; exit 4; }
  python3 -I - "$LOGS/build-$label.json" "$BIN/$sha" <<'PY' || exit 5
import json, os, shutil, sys
os.makedirs(sys.argv[2], exist_ok=True)
for line in open(sys.argv[1]):
    try: m = json.loads(line)
    except ValueError: continue
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["profile"].get("test") and "lib" in m["target"]["kind"]:
        name = m["target"]["name"]
        shutil.copy2(m["executable"], os.path.join(sys.argv[2], name))
        print("  bin", name, "->", os.path.join(sys.argv[2], name))
PY
  echo "   built $(date -u +%H:%MZ)"
}
one() { # log bin cwdsha filter args
  local log=$1 bin=$2 sha=$3; shift 3
  git checkout -q --detach "$sha"
  ( cd crates/$CRATE && timeout 900 nice -n 10 "$bin" "$@" ) >"$log" 2>&1
  echo "  $(basename "$log") exit=$? $(grep -E '^test result' "$log" | tail -1) $(state)"
  grep -E '^test .* FAILED$|panicked at' "$log" | sort -u | head -8
}
share() { # copy the body of the calibration measured by $1 under every key in $XDG
  python3 -I - "$XDG/amphora/calibration" "$1" <<'PY'
import glob, hashlib, os, sys
d, src = sys.argv[1], sys.argv[2]
texts = {f: open(f).read() for f in glob.glob(os.path.join(d, "*")) if not os.path.basename(f).startswith(".")}
mine = [f for f, t in texts.items() if t.split("\n")[2].startswith("exe " + src + " ")]
if len(mine) != 1:
    sys.exit(f"calibration: {len(mine)} files for {src} among {len(texts)}")
body = texts[mine[0]].split("\n", 3)[3]
for f, t in texts.items():
    open(f, "w").write("\n".join(t.split("\n")[:3]) + "\n" + body)
    print("  calibration", os.path.basename(f), t.split("\n")[2].split(" ")[1], "body sha256", hashlib.sha256(body.encode()).hexdigest()[:16])
PY
}
bodies() { for f in "$XDG"/amphora/calibration/*; do echo "  $(basename $f) $(sed -n 3p $f | cut -d' ' -f2) body $(tail -n +4 $f | sha256sum | cut -c1-16)"; done; }
if [ "$PHASE" = all ] || [ "$PHASE" = build ]; then
  build main "$MAIN" amphora amphora-engine
  build G "$G" amphora-engine
  build I "$I" amphora
fi
if [ "$PHASE" = all ] || [ "$PHASE" = g ]; then
  export XDG_CACHE_HOME=$XDG CRATE=engine
  EM=$BIN/$MAIN/amphora_engine EG=$BIN/$G/amphora_engine
  T=t_qt_6_30_avg_and_total_run_on_morsels_as_the_serial_loop
  echo "#### G: first run of each binary measures its own calibration $(date -u +%H:%MZ)"
  one $LOGS/g-own-main.log "$EM" "$MAIN" "$T" --test-threads=8
  one $LOGS/g-own-G.log "$EG" "$G" "$T" --test-threads=8
  bodies
  for src in main G; do
    [ $src = main ] && s=$EM || s=$EG
    echo "#### G: pass with $src's measured calibration under both keys $(date -u +%H:%MZ)"
    cp -a "$XDG/amphora/calibration" "$XDG/own-$src-$(date +%s)" 2>/dev/null
    # restore both measured bodies first so each pass copies the arm's own measurement
    [ -d "$XDG/orig" ] || cp -a "$XDG/amphora/calibration" "$XDG/orig"
    rm -rf "$XDG/amphora/calibration" && cp -a "$XDG/orig" "$XDG/amphora/calibration"
    share "$s"
    for i in 1 2 3 4 5 6; do
      for arm in main G; do
        [ $arm = main ] && b=$EM && c=$MAIN || { b=$EG; c=$G; }
        one $LOGS/g-cal$src-$arm-$i.log "$b" "$c" "$T" --test-threads=8
      done
    done
    echo "  after the pass:"; bodies
  done
fi
if [ "$PHASE" = all ] || [ "$PHASE" = i ]; then
  unset XDG_CACHE_HOME; export CRATE=amphora
  AM=$BIN/$MAIN/amphora AI=$BIN/$I/amphora
  for t in t_t1_10_r1_a_session_reads_its_write_on_a_lagging_follower a_change_another_lead_wrote_over_is_never_answered_done; do
    echo "#### I: $t alone x6 $(date -u +%H:%MZ)"
    for i in 1 2 3 4 5 6; do
      one $LOGS/i-${t:0:12}-main-$i.log "$AM" "$MAIN" "$t" --test-threads=1
      one $LOGS/i-${t:0:12}-I-$i.log "$AI" "$I" "$t" --test-threads=1
    done
  done
  echo "#### I: database:: x2 $(date -u +%H:%MZ)"
  for i in 1 2; do
    one $LOGS/i-db-main-$i.log "$AM" "$MAIN" database:: --test-threads=8
    one $LOGS/i-db-I-$i.log "$AI" "$I" database:: --test-threads=8
  done
fi
echo "J4DONE $(date -u +%H:%MZ)"
