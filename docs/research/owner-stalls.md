# The owner's stalls, measured on adacachy and jupiter

Kiln goal 6b112f14, step 1 (follow-up to the replicated-flake plan): how long a production owner's
log writes and periods stall on the hosts kiln runs it on, so step 2 can choose the leader's quorum
patience from measurements rather than ticks. The measurement is
`database::replicated::tests::production_owner_stalls_measured_on_this_host` at c5bd98e (t1/disk-stalls
on main 29be700), run with `--ignored`:
- three voters on the host's own disk under `~/.cache` (`Disk::Flushed`), on the real clock, relayed
  in-process;
- a write proposed every 20 ms at the first member, for 1,800 s;
- for each member, the time from each log write's submission to its being durable (hyper-durable's
  `Driven::flushed`), the gap between the owner's steps and between its ticks, each follower's answer
  gaps as its leader saw them (what the quorum check judges), the gaps over 200 ms, and each leader
  change with its cause.
Each series counts its samples, and the run fails if one is empty.

A step-down is the member's own, by its quorum check in its term. An election is a term it moved to.
The rest are what the member saw.

## Conditions

The plan asks for at least three runs each:
- on adacachy under load like the gates' (25-55);
- on adacachy at calm;
- on jupiter beside its shared work.

What is recorded so far:
- **jupiter** (Windows 11 Pro 10.0.26200, NTFS, beside its shared work): three runs. Windows gives no load average;
  kiln's runner snapshots during the runs showed 1 to 24 of its 32 cores idle.
- **adacachy** (Linux 7.2.9-cachyos, btrfs on NVMe, production's host): two runs, at load 39-148 and 29-155, far
  over the gates' band, with production's I/O pressure at 60-87%. Production's readiness check went
  unanswered once during the second run (2026-11-01 ~23:28Z), and the third run was stopped.
  These two are recorded as adacachy over the band, not as the loaded condition.
- The loaded (25-55) and calm runs on adacachy are not yet taken. A guarded job waits for each
  window (`stalls-adacachy-guarded.sh`). It starts a run only once its condition has held for 90 s
  with production ready, and stops it if production's readiness check fails twice in a row.

## Runs

Each run: c5bd98e, 1,800 s, a write every 20 ms. Times in ms; answer gaps are each follower's as
its leader saw them, with the count over 200 ms in brackets; the last column counts the member's own
quorum-check step-downs, the terms it moved to, and the leader changes it saw.

### jupiter, run 1

2026-11-01 22:26:52Z to 22:57:19Z, no load average (Windows). 17,213 writes acknowledged, 49 refused.

| member | write submit to durable p50/p99/p99.9/max ms | owner step gap p50/p99/p99.9/max ms | owner tick gap p50/p99/p99.9/max ms | answer gaps while leading p50/p99/p99.9/max ms (over 200 ms) | step-downs / elections / changes |
|---|---|---|---|---|---|
| 1 | 15.038/88.773/819.167/5543.762 | 0.334/21.533/39.806/7206.711 | 15.471/31.266/60.438/7206.711 | from 2: 0.495/72.914/214.972/7248.915 (82); from 3: 0.183/65.303/196.374/7246.983 (85) | 34/99/215 |
| 2 | 15.120/91.936/832.310/5544.306 | 0.800/31.443/47.325/3351.614 | 15.508/33.414/62.558/3351.614 | from 1: 0.414/61.858/153.793/1275.035 (14); from 3: 0.249/71.627/179.195/1275.080 (14) | 14/99/215 |
| 3 | 15.046/91.733/831.374/5543.838 | 0.556/31.408/47.079/4423.551 | 15.509/33.616/64.514/4423.551 | from 1: 0.343/61.220/155.119/1167.131 (13); from 2: 0.440/68.723/201.300/1167.205 (16) | 11/99/212 |

### jupiter, run 2

2026-11-01 22:57:20Z to 23:27:37Z, no load average (Windows). 14,484 writes acknowledged, 37 refused.

| member | write submit to durable p50/p99/p99.9/max ms | owner step gap p50/p99/p99.9/max ms | owner tick gap p50/p99/p99.9/max ms | answer gaps while leading p50/p99/p99.9/max ms (over 200 ms) | step-downs / elections / changes |
|---|---|---|---|---|---|
| 1 | 15.149/157.588/643.279/8942.802 | 0.515/26.756/62.441/2914.239 | 15.550/34.918/91.741/2914.239 | from 2: 0.600/110.607/266.947/2057.182 (78); from 3: 0.200/99.842/241.097/2058.505 (80) | 19/84/171 |
| 2 | 15.387/156.940/712.017/8941.751 | 0.422/31.881/64.363/3695.078 | 15.591/46.409/94.684/3695.078 | from 1: 0.399/109.001/281.793/2464.428 (164); from 3: 0.242/126.443/300.183/2465.005 (174) | 14/84/167 |
| 3 | 15.219/168.738/711.884/8941.522 | 0.602/31.987/69.145/3695.519 | 15.590/46.357/94.826/3695.519 | from 1: 0.421/77.521/189.369/1220.103 (21); from 2: 0.538/92.578/216.980/1220.457 (24) | 11/84/168 |

### jupiter, run 3

2026-11-01 23:27:38Z to 23:57:48Z, no load average (Windows). 15,430 writes acknowledged, 22 refused.

| member | write submit to durable p50/p99/p99.9/max ms | owner step gap p50/p99/p99.9/max ms | owner tick gap p50/p99/p99.9/max ms | answer gaps while leading p50/p99/p99.9/max ms (over 200 ms) | step-downs / elections / changes |
|---|---|---|---|---|---|
| 1 | 15.322/171.959/489.353/10737.105 | 0.332/24.622/67.516/6939.958 | 15.609/34.566/100.245/6939.958 | from 2: 0.363/140.651/289.518/1040.896 (167); from 3: 0.155/128.441/278.203/1043.539 (158) | 13/66/126 |
| 2 | 15.414/173.593/505.722/10737.078 | 0.525/31.704/78.201/1674.051 | 15.627/43.342/105.862/1674.051 | from 1: 0.266/108.728/298.337/582.416 (103); from 3: 0.146/125.388/331.339/581.564 (101) | 11/66/125 |
| 3 | 15.398/173.829/532.792/10642.658 | 0.354/31.645/77.548/1674.177 | 15.628/44.023/107.996/1674.177 | from 1: 0.251/94.918/272.056/1781.898 (84); from 2: 0.284/116.323/292.435/1765.556 (84) | 7/66/125 |

### adacachy, over the band, run 1

2026-11-01 22:31:44Z to 23:02:10Z, load 39.2-147.8 (sampled every 30 s). 6,682 writes acknowledged, 191 refused.

| member | write submit to durable p50/p99/p99.9/max ms | owner step gap p50/p99/p99.9/max ms | owner tick gap p50/p99/p99.9/max ms | answer gaps while leading p50/p99/p99.9/max ms (over 200 ms) | step-downs / elections / changes |
|---|---|---|---|---|---|
| 1 | 39.046/1380.130/4261.735/11716.994 | 1.120/2.943/50.679/13150.061 | 10.516/53.733/148.000/13150.061 | from 2: 0.121/183.027/914.056/9098.088 (177); from 3: 0.034/170.719/593.745/5769.679 (177) | 92/463/945 |
| 2 | 39.105/1373.516/4486.520/11187.296 | 1.120/2.717/51.329/13150.902 | 10.523/54.967/148.193/13152.135 | from 1: 0.153/179.511/372.601/9635.228 (197); from 3: 0.072/192.548/422.680/9269.470 (188) | 73/463/939 |
| 3 | 39.269/1443.419/4251.885/11188.389 | 1.120/2.729/51.161/13151.647 | 10.533/54.766/155.840/13152.702 | from 1: 0.175/159.111/514.437/13204.146 (98); from 2: 0.144/169.939/1061.737/13204.261 (90) | 61/462/941 |

### adacachy, over the band, run 2

2026-11-01 23:02:10Z to 23:32:24Z, load 28.7-155.4 (sampled every 30 s). 6,093 writes acknowledged, 244 refused.

| member | write submit to durable p50/p99/p99.9/max ms | owner step gap p50/p99/p99.9/max ms | owner tick gap p50/p99/p99.9/max ms | answer gaps while leading p50/p99/p99.9/max ms (over 200 ms) | step-downs / elections / changes |
|---|---|---|---|---|---|
| 1 | 46.190/1583.410/4831.505/11541.000 | 1.113/2.219/50.323/4446.106 | 10.918/54.318/151.289/4451.546 | from 2: 0.068/178.658/378.344/4529.613 (193); from 3: 0.017/165.427/348.209/4529.219 (189) | 127/556/1136 |
| 2 | 45.842/1604.509/4832.162/10206.490 | 1.113/2.066/50.712/16657.861 | 10.915/54.867/148.918/16664.484 | from 1: 0.095/171.329/481.000/4559.798 (105); from 3: 0.045/184.027/520.135/4559.125 (112) | 86/556/1125 |
| 3 | 45.941/1554.726/4609.127/10207.616 | 1.113/2.029/50.012/4446.109 | 10.922/54.482/149.848/4454.930 | from 1: 0.101/170.391/382.484/2492.350 (114); from 2: 0.067/180.960/496.987/2492.543 (107) | 72/556/1128 |
