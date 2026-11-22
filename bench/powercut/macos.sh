#!/usr/bin/env bash
# T-T1.4.1 (CP-T1.4, AC-T1.4.4; G8 step 4) on macOS: power cuts by powering a macOS VM off at
# random points under load, counting acknowledged commits lost. The VM runs on Apple's
# Virtualization framework through tart; `tart stop --timeout 0` ends it at once, without letting
# the guest shut down, as pulling its plug does. Each cut: the VM started, `amphora-bench
# --powercut write` run in the guest over ssh with its acknowledgements streamed to a file on the
# host, the VM stopped once the drawn count of them is in, started again, and `--powercut check`
# run in the guest on the acknowledgements received. Passes when acknowledged commits lost over
# every cut is exactly 0 (the AC: over 1,000 cuts). Every acknowledgement the host received was
# sent before the power went, so each one counts.
#
#   bench/powercut/macos.sh <vm> <amphora-bench path in the guest> [cuts]   (cuts: 1,000 unless set)
# The guest: amphora-bench built for it, ssh by key as GUEST_USER (tart's images: admin).
# Env: STORAGE=versioned (unset: the pager's); WRITERS (the step's 32); ACKS_MAX (as linux.sh);
#   GUEST_USER; WORK (~/.cache/amphora-powercut unless set).
set -euo pipefail
vm=$1
gbin=$2
cuts=${3:-1000}
writers=${WRITERS:-32}
acks_max=${ACKS_MAX:-$((256 * writers))}
user=${GUEST_USER:-admin}
work=${WORK:-$HOME/.cache/amphora-powercut}
mkdir -p "$work"
env_prefix=
[ -n "${STORAGE:-}" ] && env_prefix="AMPHORA_STORAGE=$STORAGE "
ssh_opts=(-o BatchMode=yes -o StrictHostKeyChecking=accept-new)

tpid=
stop_vm() {
  tart stop --timeout 0 "$vm" 2>/dev/null || true
  [ -n "$tpid" ] && wait "$tpid" 2>/dev/null || true
  tpid=
}
trap stop_vm EXIT

# Starts the VM and waits until its ssh answers: the guest's address once it has one.
start_vm() {
  tart run --no-graphics "$vm" > "$work/tart.log" 2>&1 &
  tpid=$!
  # tart's own wait for the guest's address; its bound is how long a boot may take before the
  # run is called broken, not a tuning.
  ip=$(tart ip --wait 600 "$vm")
  until ssh "${ssh_opts[@]}" -o ConnectTimeout=5 "$user@$ip" true 2>/dev/null; do
    kill -0 "$tpid" 2>/dev/null || { echo "the VM ended while booting" >&2; cat "$work/tart.log" >&2; exit 2; }
  done
}

: > "$work/results.jsonl"
failed=0
for cut in $(seq 1 "$cuts"); do
  start_vm
  dest="$user@$ip"
  ssh "${ssh_opts[@]}" "$dest" 'rm -rf ~/amphora-powercut && mkdir ~/amphora-powercut'
  target=$((RANDOM * 32768 + RANDOM))
  target=$((target % acks_max + 1))
  # The acknowledgements stream to the host; once the target's are in, the power goes (bash 3.2,
  # macOS's own, has no `wait -n`: the stream's reader cuts it). The rest is read to its end.
  ssh "${ssh_opts[@]}" "$dest" "${env_prefix}$gbin --powercut write --dir ~/amphora-powercut --writers $writers" 2> "$work/writer.err" |
    tee "$work/acks" | { head -n "$target" > /dev/null; tart stop --timeout 0 "$vm" 2>/dev/null; cat > /dev/null; } || true
  stop_vm
  if [ "$(wc -l < "$work/acks")" -lt "$target" ]; then
    echo "cut $cut: the writer ended before the cut" >&2
    cat "$work/writer.err" >&2
    exit 2
  fi
  start_vm
  dest="$user@$ip"
  scp -q "${ssh_opts[@]}" "$work/acks" "$dest:amphora-powercut/acks"
  if ! ssh "${ssh_opts[@]}" "$dest" "${env_prefix}$gbin --powercut check --dir ~/amphora-powercut --acks ~/amphora-powercut/acks" >> "$work/results.jsonl"; then
    failed=$((failed + 1))
    echo "cut $cut: acknowledged commits lost (results.jsonl)" >&2
  fi
  stop_vm
done
lost=$(awk -F'"lost":' 'NF > 1 { split($2, a, /[,}]/); s += a[1] } END { print s + 0 }' "$work/results.jsonl")
echo "{\"harness\":\"vm-power-off\",\"storage\":\"${STORAGE:-pager}\",\"writers\":$writers,\"cuts\":$cuts,\"cuts_failed\":$failed,\"lost\":$lost}"
[ "$failed" -eq 0 ]
