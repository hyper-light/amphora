#!/usr/bin/env bash
# T-T1.4.1 (CP-T1.4, AC-T1.4.4; G8 step 4) on Linux: power cuts by dm-flakey at random points under
# load, counting acknowledged commits lost. Each cut: a fresh ext4 on a device-mapper device over a
# loop device; `amphora-bench --powercut write` commits from WRITERS connections, its
# acknowledgements going to a file off the device. At the cut the device is suspended without
# flushing the file system (`dmsetup suspend --nolockfs`), the acknowledgements so far are the ones
# counted, and the device's table turns to dm-flakey's drop_writes, so nothing written from then on
# reaches it (xfstests' _flakey_drop_and_remount, the same way); the writer is killed, the file
# system unmounted, the device made whole again and remounted, and `--powercut check` counts the
# acknowledged ids missing. Passes when acknowledged commits lost over every cut is exactly 0 (the
# AC: over 1,000 cuts).
#
# What it models: what reached the device survives and what the kernel hadn't written doesn't. A
# write the device took that no flush made durable still survives here (the loop device's backing
# file keeps it); a drive's volatile cache lost is the macOS harness's VM power-off (macos.sh).
#
# Needs root, dmsetup, losetup, blockdev, mkfs.ext4 and bash 5.1 (SRANDOM).
#   sudo bench/powercut/linux.sh <amphora-bench binary> [cuts]    (cuts: the AC's 1,000 unless set)
# Env: STORAGE=versioned (the versioned storage; unset: the pager's); WRITERS (the step's 32);
#   ACKS_MAX (a cut lands after a uniformly drawn count of acknowledgements from 1 to ACKS_MAX:
#   unless set, `amphora-bench --powercut span`, two of the WAL's checkpoint cycles by the engine's
#   own wal_autocheckpoint, so cuts land before, across and after a checkpoint);
#   WORK (the image, the acknowledgements and results.jsonl: ~/.cache/amphora-powercut unless set,
#   never /tmp, a RAM disk on some hosts).
set -euo pipefail
bin=$(realpath "$1")
cuts=${2:-1000}
writers=${WRITERS:-32}
work=${WORK:-$HOME/.cache/amphora-powercut}
here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
name=amphora-powercut-$$
mnt=$work/mnt
img=$work/disk.img
mkdir -p "$mnt"
storage=()
[ -n "${STORAGE:-}" ] && storage=(AMPHORA_STORAGE="$STORAGE")
acks_max=${ACKS_MAX:-}
if [ -z "$acks_max" ]; then
  rm -rf "$work/span" && mkdir -p "$work/span"
  acks_max=$(env "${storage[@]}" "$bin" --powercut span --dir "$work/span")
  rm -rf "$work/span"
fi

loop=
cleanup() {
  umount "$mnt" 2>/dev/null || true
  dmsetup remove "$name" 2>/dev/null || true
  [ -n "$loop" ] && losetup -d "$loop" 2>/dev/null || true
  rm -f "$img"
}
trap cleanup EXIT

# The image is sparse: half the room free where it lies, so the run never fills it.
avail=$(df --output=avail -B1 "$work" | tail -1)
rm -f "$img"
truncate -s $((avail / 2)) "$img"
loop=$(losetup -f --show "$img")
sectors=$(blockdev --getsz "$loop")
linear="0 $sectors linear $loop 0"
# dm-flakey's table: up 0 s, down 1 s, writes dropped (xfstests' FLAKEY_TABLE_DROP shape, common/dmflakey).
# With no up interval the device is down at every moment (dm-flakey is down while the seconds since
# the table loaded, modulo up + down, are at least up: always, when up is 0), so the down length
# only has to be one that dm-flakey takes: it refuses up + down = 0, and 1 is the smallest.
flakey="0 $sectors flakey $loop 0 0 1 1 drop_writes"
: > "$work/results.jsonl"
failed=0
for cut in $(seq 1 "$cuts"); do
  dmsetup create "$name" --table "$linear"
  mkfs.ext4 -q -F "/dev/mapper/$name"
  mount "/dev/mapper/$name" "$mnt"
  : > "$work/acks"
  env "${storage[@]}" "$bin" --powercut write --dir "$mnt" --writers "$writers" > "$work/acks" 2> "$work/writer.err" &
  wpid=$!
  target=$((SRANDOM % acks_max + 1))
  # Until the target's acknowledgements are in (or the writer ends).
  tail -n +1 -F --pid="$wpid" "$work/acks" 2>/dev/null | head -n "$target" > /dev/null || true
  if ! kill -0 "$wpid" 2>/dev/null; then
    echo "cut $cut: the writer ended before the cut" >&2
    cat "$work/writer.err" >&2
    exit 2
  fi
  dmsetup suspend --nolockfs "$name"
  # The acknowledgements the cut counts: those in before the device stopped taking writes.
  acked=$(wc -l < "$work/acks")
  head -n "$acked" "$work/acks" > "$work/acks.cut"
  dmsetup load "$name" --table "$flakey"
  dmsetup resume "$name"
  kill -9 "$wpid" 2>/dev/null || true
  wait "$wpid" 2>/dev/null || true
  umount "$mnt"
  dmsetup suspend "$name"
  dmsetup load "$name" --table "$linear"
  dmsetup resume "$name"
  mount "/dev/mapper/$name" "$mnt"
  if ! env "${storage[@]}" "$bin" --powercut check --dir "$mnt" --acks "$work/acks.cut" >> "$work/results.jsonl"; then
    failed=$((failed + 1))
    echo "cut $cut: acknowledged commits lost (results.jsonl)" >&2
  fi
  umount "$mnt"
  dmsetup remove "$name"
done
"$here/../power_nemesis.sh" tally dm-flakey "${STORAGE:-pager}" "$writers" "$cuts" "$failed" "$work/results.jsonl"
