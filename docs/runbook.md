# Running Amphora on three machines (CP-T1.13)

Amphora on its own: three members of one replicated database, one per machine, each a service of
its user's own. This is the runbook for setting them up, checking them, running the fire drills and
taking them down. The server is `amphora serve` (`crates/amphora/src/serve.rs`); the scripts are
`scripts/cluster.sh` and `scripts/drills.sh`.

## What you need

- Three machines that reach each other on one TCP port each, Linux (systemd) or macOS.
- ssh from the machine you run the scripts on to each of them (`ssh -o BatchMode=yes HOST true`).
- The `amphora` binary built for those machines: `cargo build --release --locked -p amphora`
  (target/release/amphora). Linux and macOS members need their own builds.

Nothing here touches kiln: the binary goes to `~/.local/lib/amphora/amphora`, configs to
`~/.config/amphora/`, data to `~/.local/share/amphora/member-N/`, and the services are
`amphora-member-N.service` (Linux, `systemctl --user`) and `dev.amphora.member-N` (macOS,
`~/Library/LaunchAgents`).

## Install

```sh
scripts/cluster.sh install --bin target/release/amphora --class persisted --deadline-ms MS \
  [--drills on] [--spare HOST4:PORT] HOST1:PORT HOST2:PORT HOST3:PORT
```

- `--class persisted`: a write is answered once a majority of members hold it on disk.
  `--class replicated --spanning N`: answered once N members on N fates hold it, before the
  leader flushes (docs/design/replication.md). Each member is its own fate, so put each on its own
  machine.
- `--deadline-ms`: how long one request, connect or open may take. Pick it from the slowest
  machine's disk and the link's round trip; nothing here picks it.
- `--drills on`: the members take `cut` (the split drill). Leave it off in production.
- The script makes one secret (48 random bytes) and copies it to each machine's
  `~/.config/amphora/secret`, readable by its owner only. Every connection proves it holds the
  secret (HMAC-SHA256 over a random challenge); the secret never crosses the link.
- On Linux the unit stops at logout unless linger is on: `sudo loginctl enable-linger $USER`
  (the script says so when it isn't).

Three members on one machine (a trial): name the same host three times on three ports.

## Check

```sh
scripts/cluster.sh health HOST1:PORT 1     # or, on any member: amphora health ~/.config/amphora/member-1.conf
```

Healthy when every member answers, all name the same leader, each holds the three voters and none
is fenced or cut. Exit 0 healthy, 1 not, 2 can't ask. One member: `amphora ask CONF N status`.

To wait for a leader (a client that must write, while the group elects): `amphora ask CONF N leader
WAIT_MS` answers `leader=ID addr=HOST:PORT` once member N knows its group's leader, or is refused
once WAIT_MS pass. It wakes on the step that names the leader, so a client needn't retry writes a
follower refuses (`serve::Client::leader` in code).

Logs: `journalctl --user -u amphora-member-N` (Linux), `~/Library/Logs/amphora-member-N.log`
(macOS). A member whose replica fences exits with the reason; its service manager starts it again
on its data (`Restart=always`, `KeepAlive`).

## Fire drills (T-T1.13.1, AC-T1.13.5)

```sh
AMPHORA=~/.local/lib/amphora/amphora scripts/drills.sh all --config ~/.config/amphora/member-1.conf \
  --writers N --seconds S --fault-at-s A --fault-s F --catch-up-s C HOST1:PORT HOST2:PORT HOST3:PORT
```

Each drill runs `amphora drill load` (N closed-loop writers for S seconds, each writing its own
sequence 1, 2, 3, … and sending a refused or unanswered write again unchanged), faults the leader
(or `--victim N`) A seconds in, for F seconds, waits for the cluster to be healthy again, then runs
`amphora drill check`: every writer's acknowledged writes counted on every member's own copy.
Lost must be 0; the script exits 1 otherwise.

- `kill`: kill -9, kept down F seconds, started again (a node powered off).
- `split`: the victim's every record in and out dropped at its transport for F seconds (`cut`;
  needs `--drills on`). A split by the OS (iptables, pf) needs root and is the same to the members.
- `stall`: the victim's process stopped (SIGSTOP) for F seconds: its disk, network and clock stop
  together, the worst a stalled disk does to a member. A stall of the disk alone (Linux
  `dmsetup suspend`) needs root and a device-mapper volume.
- `--local`: the members are processes the script starts from the config's folder (three on one
  machine, no services); `stall` is refused on Windows (no SIGSTOP).

## A region lost, and replacing a member (T-T1.13.1)

Each member is its own region (its own machine). When one is lost for good, the two left still
hold the majority and keep taking writes; replace the lost one with a spare:

1. Name the spare in every member's `members` (`4@HOST4:PORT`), its own config with `voters =` the
   current voters (it starts as a learner), and restart the members one at a time. Or set it up
   with the cluster from the start: `scripts/cluster.sh install ... --spare HOST4:PORT HOST1:PORT
   HOST2:PORT HOST3:PORT` writes every config with the spare in `members` and `voters = 1,2,3`.
2. `amphora member CONFIG add 4`, then `amphora member CONFIG promote 4` once it has caught up
   (`amphora ask CONFIG 4 status`: its `applied` reaches the leader's).
3. `amphora member CONFIG remove 3` (the lost one), then set `voters = 1,2,4` in every config:
   `amphora health` asks the voters the config names.

`amphora member` sends the change to whichever voter says it leads. `demote` makes a voter a
learner. It tries each voter until the leader makes the change: a follower refuses, a lost member
doesn't answer. The drill under load is `scripts/drills.sh region` with the spare's HOST:PORT
fourth (the victim killed and never started again, the spare added, caught up and promoted, the
victim removed, the count on the new voters); `cargo test -p amphora --test serve a_region_lost`
is its test.

## Uninstall

```sh
scripts/cluster.sh uninstall HOST1:PORT HOST2:PORT HOST3:PORT [HOST4:PORT]   # services and configs; data kept
```

## What isn't done

- The link isn't encrypted: run the members on a network you trust, or under a tunnel.
- Mutual TLS on the links waits on the network listener's TLS (`transport/network.rs`, lane
  split-merge): serve takes its credentials and rules once that lands, so there is one TLS path.
- kiln doesn't open these members yet: kiln reaches Amphora through its conversation-store seam
  (kiln branch `t1/lane-vendor-amphora-*`), in process. A client for `amphora serve` in that seam
  is the next step, and production kiln on it is Ada's word.
