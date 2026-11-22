#!/usr/bin/env bash
# Amphora on its own across three machines (CP-T1.13; AC-T1.13.1, T-T1.13.2): sets each member up as
# a service of the user's own (a systemd user unit on Linux, a LaunchAgent on macOS), checks them,
# and takes them down. It touches only what it names amphora-member-N (units, plists, configs, data):
# never kiln's units, never ~/.local/bin. docs/runbook.md says how to run it and what each step does.
#
# usage: scripts/cluster.sh install --bin BIN --class persisted|replicated --deadline-ms MS
#                                    [--spanning N] [--drills on|off] [--service auto|none]
#                                    [--spare HOST4:PORT] HOST1:PORT HOST2:PORT HOST3:PORT
#        scripts/cluster.sh health HOST:PORT N        (member N's host asks every member)
#        scripts/cluster.sh uninstall HOST1:PORT HOST2:PORT HOST3:PORT [HOST4:PORT]   (keeps the data)
#   HOSTn:PORT  each member's address as its peers reach it; member n listens there. A host that is
#               this machine (localhost, 127.0.0.1, `hostname`) is set up here; any other by ssh.
#               One host three times on three ports is three members on one machine.
#   --bin       the amphora binary built for those machines (cargo build --release -p amphora).
#   --service   none writes the configs and the secret only (a machine without systemd or launchd,
#               or members run by hand: `amphora serve CONFIG`).
#   --spare     a fourth member, 4, set up beside the three as a learner: every config names it in
#               `members` and says `voters = 1,2,3`, so it holds no vote until `amphora member
#               CONFIG add 4` and `promote 4` (docs/runbook.md, "A region lost"; drills.sh region).
# Every size and duration is an argument: nothing is picked here.
set -euo pipefail
LIB='$HOME/.local/lib/amphora' CONF='$HOME/.config/amphora' DATA='$HOME/.local/share/amphora'
die() { echo "cluster.sh: $*" >&2; exit 2; }
here() { [[ "$1" == localhost || "$1" == 127.0.0.1 || "$1" == "$(hostname)" || "$1" == "$(hostname -s 2>/dev/null)" ]]; }
# Runs stdin as a bash script on HOST.
on() { if here "$1"; then bash -s; else ssh -o BatchMode=yes "$1" bash -s; fi; }
put() { # put HOST LOCAL REMOTE_DIR NAME
  if here "$1"; then mkdir -p "$(eval echo "$3")" && cp "$2" "$(eval echo "$3")/$4"; else ssh -o BatchMode=yes "$1" "mkdir -p $3" && scp -q "$2" "$1:${3#\$HOME/}/$4"; fi
}

install() {
  local bin='' class='' deadline='' spanning='' drills=off service=auto spare='' addrs=()
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --bin) bin=$2; shift 2 ;; --class) class=$2; shift 2 ;; --deadline-ms) deadline=$2; shift 2 ;;
      --spanning) spanning=$2; shift 2 ;; --drills) drills=$2; shift 2 ;; --service) service=$2; shift 2 ;;
      --spare) spare=$2; shift 2 ;; --*) die "unknown flag $1" ;; *) addrs+=("$1"); shift ;;
    esac
  done
  [[ -n $bin && -n $class && -n $deadline ]] || die "install needs --bin, --class and --deadline-ms"
  [[ ${#addrs[@]} -eq 3 ]] || die "install takes three HOST:PORT members, not ${#addrs[@]}"
  [[ -x $bin ]] || die "no binary at $bin"
  [[ -n $spare ]] && addrs+=("$spare")
  local members='' n count=${#addrs[@]}
  for n in $(seq 1 "$count"); do members+="${members:+,}$n@${addrs[$((n - 1))]}"; done
  # One secret for the cluster, made here once (48 random bytes as base64: past RFC 2104's 32-byte
  # minimum for HMAC-SHA256), readable by its owner only.
  local secret; secret=$(mktemp "${TMPDIR:-$HOME}/amphora-secret.XXXXXX")
  trap 'rm -f "$secret"' RETURN
  (umask 077; head -c 48 /dev/urandom | base64 >"$secret")
  for n in $(seq 1 "$count"); do
    local addr=${addrs[$((n - 1))]} host=${addrs[$((n - 1))]%:*}
    echo "== member $n on $addr" >&2
    put "$host" "$bin" "$LIB" amphora
    put "$host" "$secret" "$CONF" secret
    on "$host" <<EOF
set -euo pipefail
chmod 700 "$CONF"; chmod 600 "$CONF/secret"; chmod 755 "$LIB/amphora"; mkdir -p "$DATA/member-$n"
cat >"$CONF/member-$n.conf" <<CONF
# Amphora member $n (scripts/cluster.sh install; docs/runbook.md)
node = $n
members = $members
data = $DATA/member-$n
class = $class
${spanning:+spanning = $spanning}
${spare:+voters = 1,2,3}
secret_file = $CONF/secret
deadline_ms = $deadline
drills = $drills
CONF
case "$service:\$(uname -s)" in
  none:*) echo "member $n: config only: $LIB/amphora serve $CONF/member-$n.conf" ;;
  auto:Linux)
    mkdir -p "\$HOME/.config/systemd/user"
    cat >"\$HOME/.config/systemd/user/amphora-member-$n.service" <<UNIT
[Unit]
Description=Amphora member $n (scripts/cluster.sh)
After=network-online.target

[Service]
ExecStart=$LIB/amphora serve $CONF/member-$n.conf
Restart=always

[Install]
WantedBy=default.target
UNIT
    systemctl --user daemon-reload
    systemctl --user enable --now amphora-member-$n.service
    loginctl show-user "\$USER" -p Linger 2>/dev/null | grep -q yes || echo "member $n: the unit stops at logout until linger is on: sudo loginctl enable-linger \$USER" >&2 ;;
  auto:Darwin)
    plist="\$HOME/Library/LaunchAgents/dev.amphora.member-$n.plist"
    mkdir -p "\$HOME/Library/LaunchAgents" "\$HOME/Library/Logs"
    cat >"\$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>dev.amphora.member-$n</string>
  <key>ProgramArguments</key><array><string>$LIB/amphora</string><string>serve</string><string>$CONF/member-$n.conf</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardErrorPath</key><string>\$HOME/Library/Logs/amphora-member-$n.log</string>
</dict></plist>
PLIST
    launchctl bootout "gui/\$(id -u)/dev.amphora.member-$n" 2>/dev/null || true
    launchctl bootstrap "gui/\$(id -u)" "\$plist" ;;
  *) echo "member $n: no service manager on \$(uname -s): run $LIB/amphora serve $CONF/member-$n.conf under one" >&2; exit 1 ;;
esac
EOF
  done
  echo "installed; check: scripts/cluster.sh health ${addrs[0]} 1" >&2
}

health() {
  [[ $# -eq 2 ]] || die "health takes HOST:PORT N"
  on "${1%:*}" <<<"$LIB/amphora health $CONF/member-$2.conf"
}

uninstall() {
  [[ $# -eq 3 || $# -eq 4 ]] || die "uninstall takes three HOST:PORT members and the spare's, if any"
  local n=0 addr
  for addr in "$@"; do
    n=$((n + 1))
    on "${addr%:*}" <<EOF
case "\$(uname -s)" in
  Linux) systemctl --user disable --now amphora-member-$n.service 2>/dev/null || true; rm -f "\$HOME/.config/systemd/user/amphora-member-$n.service"; systemctl --user daemon-reload ;;
  Darwin) launchctl bootout "gui/\$(id -u)/dev.amphora.member-$n" 2>/dev/null || true; rm -f "\$HOME/Library/LaunchAgents/dev.amphora.member-$n.plist" ;;
esac
rm -f "$CONF/member-$n.conf"
echo "member $n: service removed; its data stays in $DATA/member-$n"
EOF
  done
}

cmd=${1:-}; shift || true
case "$cmd" in
  install) install "$@" ;; health) health "$@" ;; uninstall) uninstall "$@" ;;
  *) sed -n '2,20p' "$0" >&2; exit 2 ;;
esac
