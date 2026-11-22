#!/usr/bin/env bash
# Fire drills under load (CP-T1.13; T-T1.13.1, AC-T1.13.5): writers run through the three members
# while one member is killed, split off or stalled; once it heals and the cluster is healthy again,
# every acknowledged write is counted on every member. Lost must be 0. docs/runbook.md, "Fire drills".
#
# usage: scripts/drills.sh DRILL --config CONF --writers N --seconds S --fault-at-s A --fault-s F
#                          --catch-up-s C [--victim leader|N] [--local] HOST1:PORT HOST2:PORT HOST3:PORT
#   DRILL       kill (kill -9, kept down F s, started again: a node powered off), split (the victim's
#               every record in and out dropped at its transport for F s: `cut`, the members' config
#               must say drills = on), stall (the victim's process stopped for F s, SIGSTOP/SIGCONT:
#               its disk, its network and its clock stop together, the worst a stalled disk does),
#               disk (the victim's log disk cut: every read, write and flush of it fails, `disk on`,
#               drills = on; its replica fences and the member ends; kept down F s and started
#               again, its disk back, it catches up from its own durable log; as services, the
#               service manager starts it again as soon as it ends), roll (a rolling restart: each
#               member in turn, the leader last, stopped as its service manager stops it (SIGTERM),
#               kept down F s, started again, and caught up and the cluster healthy before the
#               next; --victim is ignored),
#               region (the victim lost for good, a region gone: killed and never started again; the
#               spare, a fourth HOST:PORT named in every member's `members` and started as a learner
#               by `voters = 1,2,3`, added, caught up, promoted, the victim removed; the count then
#               runs on the new voters, from CONF's copy region-after.conf), or all (kill, split,
#               stall, disk, roll).
#   --config    any member's config, on this machine: the members, the secret and the deadline.
#   --victim    the member to fault (default: the leader when the fault starts).
#   --local     the members are processes this script starts here from CONF's folder's
#               member-N.conf (three on one machine); without it they're cluster.sh's services.
#   --writers/--seconds  the load (amphora drill load); --fault-at-s after its start the fault
#               begins, --fault-s it lasts; --catch-up-s how long the count waits for a member.
# Every size and duration is an argument: nothing is picked here. Exit 0 only when every drill
# lost nothing.
set -euo pipefail
die() { echo "drills.sh: $*" >&2; exit 2; }
drill=${1:-}; shift || true
conf='' writers='' seconds='' at='' fault='' catchup='' victim=leader local=0 addrs=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --config) conf=$2; shift 2 ;; --writers) writers=$2; shift 2 ;; --seconds) seconds=$2; shift 2 ;;
    --fault-at-s) at=$2; shift 2 ;; --fault-s) fault=$2; shift 2 ;; --catch-up-s) catchup=$2; shift 2 ;;
    --victim) victim=$2; shift 2 ;; --local) local=1; shift ;;
    --*) die "unknown flag $1" ;; *) addrs+=("$1"); shift ;;
  esac
done
[[ -n $conf && -n $writers && -n $seconds && -n $at && -n $fault && -n $catchup ]] || die "every flag but --victim and --local is required"
[[ ${#addrs[@]} -eq 3 || ( $drill == region && ${#addrs[@]} -eq 4 ) ]] || die "three HOST:PORT members (region: and the spare's), not ${#addrs[@]}"
BIN=${AMPHORA:-$HOME/.local/lib/amphora/amphora}
[[ -x $BIN || -x $BIN.exe ]] || die "no amphora at $BIN (AMPHORA=path)"
here() { [[ "$1" == localhost || "$1" == 127.0.0.1 || "$1" == "$(hostname)" || "$1" == "$(hostname -s 2>/dev/null)" ]]; }
on() { if here "$1"; then bash -s; else ssh -o BatchMode=yes "$1" bash -s; fi; }
DIR=$(cd "$(dirname "$conf")" && pwd)
PID=()
start_local() { "$BIN" serve "$DIR/member-$1.conf" 2>>"$DIR/member-$1.log" & PID[$1]=$!; }
if [[ $local == 1 ]]; then
  for n in $(seq 1 ${#addrs[@]}); do start_local $n; done
  trap 'for p in "${PID[@]}"; do kill -9 "$p" 2>/dev/null || true; done' EXIT
fi

leader() { local n l; for n in 1 2 3; do l=$("$BIN" ask "$conf" $n status 2>/dev/null | tr ' ' '\n' | sed -n 's/^leader=//p' || true); [[ $l =~ ^[123]$ ]] && { echo "$l"; return; }; done; }
healthy() { # healthy SECONDS: until the cluster says healthy, or fails
  local end=$((SECONDS + $1))
  while ((SECONDS < end)); do "$BIN" health "$conf" >/dev/null 2>&1 && return 0; sleep 1; done
  "$BIN" health "$conf" >&2 || return 1
}
# The service's process ID on its host.
service_pid() { on "${addrs[$(($1 - 1))]%:*}" <<EOF
case "\$(uname -s)" in
  Linux) systemctl --user show -p MainPID --value amphora-member-$1.service ;;
  Darwin) launchctl print "gui/\$(id -u)/dev.amphora.member-$1" | awk '/^\tpid = /{print \$3}' ;;
esac
EOF
}
fault_on() { # fault_on DRILL N
  local host=${addrs[$(($2 - 1))]%:*}
  case "$1:$local" in
    region:*) region "$2" ;;
    kill:1) kill -9 "${PID[$2]}"; sleep "$fault"; start_local "$2" ;;
    kill:0) on "$host" <<EOF
case "\$(uname -s)" in
  Linux) systemctl --user kill -s SIGKILL amphora-member-$2.service; systemctl --user stop amphora-member-$2.service; sleep $fault; systemctl --user start amphora-member-$2.service ;;
  Darwin) p=\$(launchctl print "gui/\$(id -u)/dev.amphora.member-$2" | awk '/^\tpid = /{print \$3}'); kill -9 "\$p"; launchctl bootout "gui/\$(id -u)/dev.amphora.member-$2" 2>/dev/null || true; sleep $fault; launchctl bootstrap "gui/\$(id -u)" "\$HOME/Library/LaunchAgents/dev.amphora.member-$2.plist" ;;
esac
EOF
    ;;
    split:*) "$BIN" ask "$conf" "$2" cut on >/dev/null; sleep "$fault"; "$BIN" ask "$conf" "$2" cut off >/dev/null ;;
    stall:1)
      [[ "$(uname -s)" != MINGW* && "$(uname -s)" != MSYS* ]] || { echo "refused: stall: SIGSTOP doesn't reach a native Windows process" >&2; return 3; }
      kill -STOP "${PID[$2]}"; sleep "$fault"; kill -CONT "${PID[$2]}" ;;
    stall:0) local p; p=$(service_pid "$2"); on "$host" <<<"kill -STOP $p; sleep $fault; kill -CONT $p" ;;
    disk:1) "$BIN" ask "$conf" "$2" disk on >/dev/null || return 1; ended "$2" || return 1; sleep "$fault"; start_local "$2" ;;
    disk:0) "$BIN" ask "$conf" "$2" disk on >/dev/null || return 1; sleep "$fault" ;;
    roll:*) roll ;;
  esac
}
# ended N: until the local member N's process ends on its own (its replica fenced), at most
# --catch-up-s: woken by its end, a watchdog killing it at the deadline.
ended() {
  local p=${PID[$1]} dog rc=0
  (sleep "$catchup"; kill -9 "$p" 2>/dev/null && echo "member $1 never ended on its own" >&2) & dog=$!
  wait "$p" 2>/dev/null || true
  kill "$dog" 2>/dev/null || rc=1; wait "$dog" 2>/dev/null || true
  return $rc
}
restart() { # restart N: member N stopped as its service manager stops it, kept down F s, started
  local host=${addrs[$(($1 - 1))]%:*}
  if [[ $local == 1 ]]; then
    kill -TERM "${PID[$1]}" 2>/dev/null || true; wait "${PID[$1]}" 2>/dev/null || true; sleep "$fault"; start_local "$1"
  else on "$host" <<EOF
case "\$(uname -s)" in
  Linux) systemctl --user stop amphora-member-$1.service; sleep $fault; systemctl --user start amphora-member-$1.service ;;
  Darwin) launchctl bootout "gui/\$(id -u)/dev.amphora.member-$1" 2>/dev/null || true; sleep $fault; launchctl bootstrap "gui/\$(id -u)" "\$HOME/Library/LaunchAgents/dev.amphora.member-$1.plist" ;;
esac
EOF
  fi
}
roll() { # every member restarted in turn, the leader last, each caught up before the next
  local l n; l=$(leader)
  for n in $(printf '%s\n' 1 2 3 | grep -vx "${l:-0}") $l; do
    echo "== roll: member $n" >&2
    restart "$n" || return 1
    caught_up "$n" "$catchup" || { echo "member $n never caught up after its restart" >&2; return 1; }
    healthy "$catchup" || return 1
  done
}

status_of() { "$BIN" ask "$conf" "$1" status 2>/dev/null | tr ' ' '
' | sed -n "s/^$2=//p" || true; }
caught_up() { # caught_up N SECONDS: until member N has applied what the leader has
  local end=$((SECONDS + $2)) l
  while ((SECONDS < end)); do
    local have want; l=$(leader); have=$(status_of "$1" applied); want=$(status_of "${l:-0}" applied)
    [[ -n $l && -n $have && -n $want ]] && ((have >= want)) && return 0
    sleep 1
  done
  return 1
}
change() { # change WHAT N: until a leader makes it (one is elected once the victim's gone), or fails
  local end=$((SECONDS + catchup))
  until "$BIN" member "$conf" "$1" "$2" >&2; do ((SECONDS < end)) || return 1; sleep 1; done
}
region() { # region N: member N lost for good, the spare (4) in its place
  local host=${addrs[$(($1 - 1))]%:*}
  if [[ $local == 1 ]]; then kill -9 "${PID[$1]}"; else on "$host" <<EOF
case "\$(uname -s)" in
  Linux) systemctl --user kill -s SIGKILL amphora-member-$1.service; systemctl --user stop amphora-member-$1.service ;;
  Darwin) launchctl bootout "gui/\$(id -u)/dev.amphora.member-$1" 2>/dev/null || true ;;
esac
EOF
  fi
  change add 4 || return 1
  caught_up 4 "$catchup" || { echo "the spare never caught up" >&2; return 1; }
  change promote 4 && change remove "$1" || return 1
  local voters; voters=$(printf '%s
' 1 2 3 4 | grep -vx "$1" | paste -sd, -)
  grep -v '^ *voters *=' "$conf" >"$DIR/region-after.conf"; echo "voters = $voters" >>"$DIR/region-after.conf"
  conf="$DIR/region-after.conf"
}

one() { # one DRILL: load, the fault inside it, heal, count
  local d=$1 run="$1-$(date -u +%Y%m%dT%H%M%SZ)" ledger
  ledger="$DIR/$run.ledger"
  healthy "$catchup" || { echo "FAILED $d: unhealthy before the drill" >&2; return 1; }
  "$BIN" drill load "$conf" --run "$run" --writers "$writers" --seconds "$seconds" --ledger "$ledger" >"$DIR/$run.load" &
  local load=$!
  sleep "$at"
  local v=$victim; [[ $v == leader ]] && v=$(leader)
  [[ $v =~ ^[123]$ ]] || { wait $load; echo "FAILED $d: no victim (leader unknown)" >&2; return 1; }
  echo "== $d: member $v for ${fault}s under load" >&2
  local f=0; fault_on "$d" "$v" || f=$?
  wait $load || { echo "FAILED $d: the load" >&2; return 1; }
  [[ $f == 3 ]] && { echo "refused $d" >&2; return 0; }
  ((f == 0)) || { echo "FAILED $d: the fault" >&2; return 1; }
  cat "$DIR/$run.load"
  healthy "$catchup" || { echo "FAILED $d: unhealthy after it healed" >&2; return 1; }
  "$BIN" drill check "$conf" --ledger "$ledger" --catch-up-s "$catchup"
}

failed=0
case "$drill" in
  kill | split | stall | disk | roll | region) one "$drill" || failed=1 ;;
  all) for d in kill split stall disk roll; do one "$d" || failed=1; done ;;
  *) sed -n '2,31p' "$0" >&2; exit 2 ;;
esac
exit $failed
