# Fast acknowledgement: what a flush and a round trip cost here (2026-11-01)

Measurements for docs/design/replication.md, "Fast acknowledgement: the replicated class". Kiln goal
1b2c66ae884889609ce5248729385a0a, plan 01a198d660e272fa934856a45b9a73c0, step 1. Every run was made through kiln's
MCP run. The probes are flushprobe.py and rttprobe.py, at commit e61a5bd of ~/.cache/claude-scratch/probe-carrier.

## What a commit waits for

- **A classic commit**, Raft's (raft.pdf Figure 2: "Updated on stable storage before responding to RPCs"), waits for a
  quorum's flushes plus a round trip.
- **A held commit** (hyper-raft R-8, raft.md §3.7 at 4d72e19b) waits for a round trip to a held quorum whose fates span
  what the owner asks, or for the leader's own write plus one follower's memory, whichever comes first.
- **Amphora's state machine** flushes its WAL before apply answers (replication.rs `Machine::advance`). So until that
  changes, every answer also waits for one local flush on the proposing member.

## The flush: a 4 KiB append and the flush Amphora's engine uses

Amphora's engine flushes with fdatasync on Linux and `fcntl(F_FULLFSYNC)` on Apple's systems (crates/engine/io/unix.rs).
flushprobe.py appends 4,096 bytes and flushes, 2,000 times, in a file of its own under ~/.cache.

| host | OS | file system | flush | load | count | p50 | p99 | p99.9 | max | when (UTC) |
|---|---|---|---|---|---|---|---|---|---|---|
| adas-mac-studio-3 | Darwin 25.2.0 | apfs | F_FULLFSYNC | 2.03-2.30 | 2,000 | 3.928 ms | 6.014 ms | 7.892 ms | 10.917 ms | 2026-11-01 20:47:36 |
| adacachy | Linux 7.2.9-2-cachyos | btrfs (nvme1n1p2, /@home) | fdatasync | 75.96-78.50 | 2,000 | 21.324 ms | 1,340.648 ms | 4,271.056 ms | 5,625.591 ms | 2026-11-01 20:49:49 |
| kiln-a094a12d (fedora-2) | Linux 6.19.10-300.fc44 | btrfs (nvme1n1p3, /home) | fdatasync | 15.48-16.82 | 2,000 | 8.836 ms | 18.266 ms | 25.415 ms | 209.365 ms | 2026-11-01 21:09:48 |

Still to come: Linux at calm (load under 2). fedora-2 ran at load 15-17 under two CI jobs, and adacachy has not been
calm today; its load has run 30-190 under other sessions' builds.

## The round trip: a 64-byte datagram and its answer

rttprobe.py sends 2,000 datagrams one at a time and times each answer from a user-level echo. ICMP ping is the kernel's
own echo, so it shows the link without a user process at the far end.

**Every host measured reaches the others over Wi-Fi.** studio-3's Ethernet (en0) is inactive and its default route is
en1. adacachy routes 192.168.1.0/24 over wlan0. jupiter's 192.168.1.75 is its "Wireless LAN adapter Wi-Fi".
adas-macbook-pro-2's en0 is Wi-Fi. fedora-2 (kiln-a094a12d) reaches the LAN over wlo1.

| from | to | probe | load at the sender | answered / lost | p50 | p99 | p99.9 | max | when (UTC) |
|---|---|---|---|---|---|---|---|---|---|
| adacachy | studio-3 (192.168.1.83) | UDP echo | 45.14-77.09 | 1,954 / 46 | 37.702 ms | 374.112 ms | 482.070 ms | 614.142 ms | 20:53:06 |
| adas-macbook-pro-2 | studio-3 | UDP echo | 16.14-48.76 | 1,917 / 83 | 60.763 ms | 541.630 ms | 808.562 ms | 896.713 ms | 21:02:19 |
| jupiter | studio-3 | UDP echo (PowerShell) | 30 of 32 cores idle | 1,996 / 4 | 27.570 ms | 436.181 ms | 766.114 ms | 799.984 ms | 21:05:47 |

| from | to | ICMP ping | min | avg | max |
|---|---|---|---|---|---|
| jupiter | studio-3 | 40 sent, 40 received | 10 ms | 242 ms | 543 ms |
| studio-3 | jupiter | 40 sent, 40 received | 39.256 ms | 223.898 ms | 567.725 ms |
| adacachy | jupiter | 200 sent, 200 received | 2.641 ms | 17.097 ms (mdev 43.496) | 283.104 ms |
| fedora-2 | jupiter | 200 sent, 200 received | 3.836 ms | 16.669 ms (mdev 65.181) | 717.691 ms |

Still to come: a wired pair, if one exists.

## What they say so far

- On loaded btrfs, one flush's tail (p99 1.34 s, max 5.6 s) is far longer than the Wi-Fi round trip's (p99 374-542 ms).
  There, a commit held across fates would answer sooner than one waiting for that disk.
- btrfs's flush is slow even at moderate load: fedora-2's p50 is 8.8 ms and p99 18.3 ms at load 15-17, near this
  Wi-Fi's typical round trip (ping average 16.7 ms), with a max of 209 ms against the link's 718 ms.
- On a calm Mac, the flush (p99 6.0 ms) is far shorter than this Wi-Fi's round trip (p99 374-542 ms). There a held
  commit would answer later, not sooner, unless the members share a wired network. R-8 commits on whichever quorum
  comes first (raft.md §3.7: "never slower than the classic commit"), so where it is slower it is simply not used.
- Wi-Fi drops datagrams (4-83 of 2,000 here). A held commit then waits for the transport's resend, and a member whose
  answers stall is suspected and no longer counted (`suspect_held`).
- The kernel's ping shows the link itself is slow and bursty here (adacachy to jupiter: min 2.6 ms, max 283 ms). The
  user-level probes add each host's scheduling on top (adacachy's p50 37.7 ms at load 77).
