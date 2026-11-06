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
- **adacachy** (Linux 7.2.9-cachyos, btrfs on NVMe, production's host): seven runs.
  - Two at c5bd98e, at load 39-148 and 29-155, far over the gates' band, with production's I/O pressure at
    60-87%. Production's readiness check went unanswered once during the second (2026-11-01 ~23:28Z), and a
    third was stopped. These two are recorded as adacachy over the band, not as the loaded condition.
  - Five at 93e311f and bc3b203 by the guarded job (`stalls-adacachy-guarded.sh`). It starts a run only once the
    load has held within 25-55 for 90 s with production ready, and stops it if production's readiness check fails
    twice in a row, which stopped four earlier tries. adacachy's load moves fast: each run went above 55 at times.
    A run counts as the loaded condition when the median of its load samples (every 30 s) is within 25-55, its
    range beside. Three do (medians 49.9, 52.3 and 40.3), and two are over the band (medians 61.8 and 58.7).
- The calm condition is dropped (Ada, 2026-11-04). adacachy was never calm from 2026-11-02 01:22Z on: its calm
  watchers found no load under 2, and the load sampled there ran from 20.9 to 469. On fedora, where Ada moved the
  calm runs on 2026-11-03, a guarded job (`stalls-fedora-calm.sh`: load under 2 with nothing compiling, at most 3
  tasks running and the disk under a quarter busy, held 90 s) waited from 2026-11-03 03:01Z to 2026-11-04 06:18Z.
  It started three runs, at 16:45Z, 17:23Z and 19:39Z, and stopped each within 5.5-6.5 minutes when other work
  raised the load again (to 7.5, 21.3 and 23.0), and no other window came. So step 2's decision rests on the
  loaded adacachy runs and jupiter's, where the stalls occur.

## Runs

Each run: c5bd98e, 93e311f or bc3b203 (which add only this record), 1,800 s, a write every 20 ms. Times in ms;
answer gaps are each follower's as its leader saw them, with the count over 200 ms in brackets; the last column
counts the member's own quorum-check step-downs, the terms it moved to, and the leader changes it saw.

### jupiter, run 1 (c5bd98e)

2026-11-01 22:26:52Z to 22:57:19Z, no load average (Windows). 17,213 writes acknowledged, 49 refused.

| member | write submit to durable p50/p99/p99.9/max ms | owner step gap p50/p99/p99.9/max ms | owner tick gap p50/p99/p99.9/max ms | answer gaps while leading p50/p99/p99.9/max ms (over 200 ms) | step-downs / elections / changes |
|---|---|---|---|---|---|
| 1 | 15.038/88.773/819.167/5543.762 | 0.334/21.533/39.806/7206.711 | 15.471/31.266/60.438/7206.711 | from 2: 0.495/72.914/214.972/7248.915 (82); from 3: 0.183/65.303/196.374/7246.983 (85) | 34/99/215 |
| 2 | 15.120/91.936/832.310/5544.306 | 0.800/31.443/47.325/3351.614 | 15.508/33.414/62.558/3351.614 | from 1: 0.414/61.858/153.793/1275.035 (14); from 3: 0.249/71.627/179.195/1275.080 (14) | 14/99/215 |
| 3 | 15.046/91.733/831.374/5543.838 | 0.556/31.408/47.079/4423.551 | 15.509/33.616/64.514/4423.551 | from 1: 0.343/61.220/155.119/1167.131 (13); from 2: 0.440/68.723/201.300/1167.205 (16) | 11/99/212 |

### jupiter, run 2 (c5bd98e)

2026-11-01 22:57:20Z to 23:27:37Z, no load average (Windows). 14,484 writes acknowledged, 37 refused.

| member | write submit to durable p50/p99/p99.9/max ms | owner step gap p50/p99/p99.9/max ms | owner tick gap p50/p99/p99.9/max ms | answer gaps while leading p50/p99/p99.9/max ms (over 200 ms) | step-downs / elections / changes |
|---|---|---|---|---|---|
| 1 | 15.149/157.588/643.279/8942.802 | 0.515/26.756/62.441/2914.239 | 15.550/34.918/91.741/2914.239 | from 2: 0.600/110.607/266.947/2057.182 (78); from 3: 0.200/99.842/241.097/2058.505 (80) | 19/84/171 |
| 2 | 15.387/156.940/712.017/8941.751 | 0.422/31.881/64.363/3695.078 | 15.591/46.409/94.684/3695.078 | from 1: 0.399/109.001/281.793/2464.428 (164); from 3: 0.242/126.443/300.183/2465.005 (174) | 14/84/167 |
| 3 | 15.219/168.738/711.884/8941.522 | 0.602/31.987/69.145/3695.519 | 15.590/46.357/94.826/3695.519 | from 1: 0.421/77.521/189.369/1220.103 (21); from 2: 0.538/92.578/216.980/1220.457 (24) | 11/84/168 |

### jupiter, run 3 (c5bd98e)

2026-11-01 23:27:38Z to 23:57:48Z, no load average (Windows). 15,430 writes acknowledged, 22 refused.

| member | write submit to durable p50/p99/p99.9/max ms | owner step gap p50/p99/p99.9/max ms | owner tick gap p50/p99/p99.9/max ms | answer gaps while leading p50/p99/p99.9/max ms (over 200 ms) | step-downs / elections / changes |
|---|---|---|---|---|---|
| 1 | 15.322/171.959/489.353/10737.105 | 0.332/24.622/67.516/6939.958 | 15.609/34.566/100.245/6939.958 | from 2: 0.363/140.651/289.518/1040.896 (167); from 3: 0.155/128.441/278.203/1043.539 (158) | 13/66/126 |
| 2 | 15.414/173.593/505.722/10737.078 | 0.525/31.704/78.201/1674.051 | 15.627/43.342/105.862/1674.051 | from 1: 0.266/108.728/298.337/582.416 (103); from 3: 0.146/125.388/331.339/581.564 (101) | 11/66/125 |
| 3 | 15.398/173.829/532.792/10642.658 | 0.354/31.645/77.548/1674.177 | 15.628/44.023/107.996/1674.177 | from 1: 0.251/94.918/272.056/1781.898 (84); from 2: 0.284/116.323/292.435/1765.556 (84) | 7/66/125 |

### adacachy, over the band, run 1 (c5bd98e)

2026-11-01 22:31:44Z to 23:02:10Z, load 39.2-147.8 (sampled every 30 s). 6,682 writes acknowledged, 191 refused.

| member | write submit to durable p50/p99/p99.9/max ms | owner step gap p50/p99/p99.9/max ms | owner tick gap p50/p99/p99.9/max ms | answer gaps while leading p50/p99/p99.9/max ms (over 200 ms) | step-downs / elections / changes |
|---|---|---|---|---|---|
| 1 | 39.046/1380.130/4261.735/11716.994 | 1.120/2.943/50.679/13150.061 | 10.516/53.733/148.000/13150.061 | from 2: 0.121/183.027/914.056/9098.088 (177); from 3: 0.034/170.719/593.745/5769.679 (177) | 92/463/945 |
| 2 | 39.105/1373.516/4486.520/11187.296 | 1.120/2.717/51.329/13150.902 | 10.523/54.967/148.193/13152.135 | from 1: 0.153/179.511/372.601/9635.228 (197); from 3: 0.072/192.548/422.680/9269.470 (188) | 73/463/939 |
| 3 | 39.269/1443.419/4251.885/11188.389 | 1.120/2.729/51.161/13151.647 | 10.533/54.766/155.840/13152.702 | from 1: 0.175/159.111/514.437/13204.146 (98); from 2: 0.144/169.939/1061.737/13204.261 (90) | 61/462/941 |

### adacachy, over the band, run 2 (c5bd98e)

2026-11-01 23:02:10Z to 23:32:24Z, load 28.7-155.4 (sampled every 30 s). 6,093 writes acknowledged, 244 refused.

| member | write submit to durable p50/p99/p99.9/max ms | owner step gap p50/p99/p99.9/max ms | owner tick gap p50/p99/p99.9/max ms | answer gaps while leading p50/p99/p99.9/max ms (over 200 ms) | step-downs / elections / changes |
|---|---|---|---|---|---|
| 1 | 46.190/1583.410/4831.505/11541.000 | 1.113/2.219/50.323/4446.106 | 10.918/54.318/151.289/4451.546 | from 2: 0.068/178.658/378.344/4529.613 (193); from 3: 0.017/165.427/348.209/4529.219 (189) | 127/556/1136 |
| 2 | 45.842/1604.509/4832.162/10206.490 | 1.113/2.066/50.712/16657.861 | 10.915/54.867/148.918/16664.484 | from 1: 0.095/171.329/481.000/4559.798 (105); from 3: 0.045/184.027/520.135/4559.125 (112) | 86/556/1125 |
| 3 | 45.941/1554.726/4609.127/10207.616 | 1.113/2.029/50.012/4446.109 | 10.922/54.482/149.848/4454.930 | from 1: 0.101/170.391/382.484/2492.350 (114); from 2: 0.067/180.960/496.987/2492.543 (107) | 72/556/1128 |

### adacachy, loaded, run 1 (93e311f)

2026-11-02 00:28:16Z to 00:58:35Z, load 28.5-159.6, median 49.9 (sampled every 30 s). 6,464 writes acknowledged,
241 refused.

| member | write submit to durable p50/p99/p99.9/max ms | owner step gap p50/p99/p99.9/max ms | owner tick gap p50/p99/p99.9/max ms | answer gaps while leading p50/p99/p99.9/max ms (over 200 ms) | step-downs / elections / changes |
|---|---|---|---|---|---|
| 1 | 53.167/1162.997/3620.196/7889.683 | 1.112/1.878/50.269/4075.419 | 10.939/55.073/126.641/4075.419 | from 2: 0.048/170.095/396.293/4138.317 (146); from 3: 0.014/160.879/363.857/4139.250 (153) | 113/522/1090 |
| 2 | 53.049/1162.569/3410.234/7889.539 | 1.112/1.485/50.582/4076.277 | 10.943/55.318/121.273/4080.726 | from 1: 0.082/161.485/533.239/3083.123 (114); from 3: 0.036/171.377/610.545/3083.333 (114) | 84/522/1090 |
| 3 | 52.934/1167.607/3620.348/7396.203 | 1.112/1.486/51.231/4076.305 | 10.942/55.954/136.865/4081.893 | from 1: 0.081/171.259/585.884/5347.205 (113); from 2: 0.034/180.727/663.218/2644.241 (117) | 79/522/1073 |

### adacachy, loaded, run 2 (93e311f)

2026-11-02 08:08:57Z to 08:39:22Z, load 38.7-72.8, median 52.3 (sampled every 30 s). 3,824 writes acknowledged, 223
refused.

| member | write submit to durable p50/p99/p99.9/max ms | owner step gap p50/p99/p99.9/max ms | owner tick gap p50/p99/p99.9/max ms | answer gaps while leading p50/p99/p99.9/max ms (over 200 ms) | step-downs / elections / changes |
|---|---|---|---|---|---|
| 1 | 59.372/2530.040/6846.210/14410.467 | 1.063/1.150/42.918/6527.271 | 10.621/48.313/121.586/6536.945 | from 2: 0.027/201.524/1417.374/4967.128 (170); from 3: 0.009/189.752/876.673/4968.192 (169) | 98/655/1205 |
| 2 | 59.400/2571.589/6846.177/14410.670 | 1.063/1.136/43.677/6521.658 | 10.618/48.772/120.714/6530.244 | from 1: 0.072/184.527/1642.788/6554.087 (107); from 3: 0.028/190.142/2286.190/6554.048 (103) | 85/655/1201 |
| 3 | 59.293/2499.193/6558.709/14410.448 | 1.063/1.132/43.835/9477.636 | 10.617/49.228/123.007/9483.032 | from 1: 0.071/190.863/1528.489/9645.032 (115); from 2: 0.018/199.963/1765.884/9645.445 (115) | 85/655/1197 |

### adacachy, over the band, run 3 (93e311f)

2026-11-02 08:45:23Z to 09:15:32Z, load 44.3-90.6, median 61.8 (sampled every 30 s). 4,382 writes acknowledged, 257
refused.

| member | write submit to durable p50/p99/p99.9/max ms | owner step gap p50/p99/p99.9/max ms | owner tick gap p50/p99/p99.9/max ms | answer gaps while leading p50/p99/p99.9/max ms (over 200 ms) | step-downs / elections / changes |
|---|---|---|---|---|---|
| 1 | 47.939/2377.840/5639.939/12739.897 | 1.070/1.601/40.870/7628.579 | 10.677/46.125/118.950/7631.121 | from 2: 0.060/188.262/1615.118/7714.853 (148); from 3: 0.018/174.954/1113.206/7711.043 (148) | 122/686/1280 |
| 2 | 47.957/2446.437/6123.641/12739.329 | 1.070/1.432/41.129/7631.754 | 10.671/46.408/118.857/7640.442 | from 1: 0.122/168.535/707.919/3587.127 (104); from 3: 0.048/183.394/1216.057/3587.217 (103) | 97/686/1279 |
| 3 | 47.785/2467.817/5842.871/12739.537 | 1.070/1.424/41.273/7629.418 | 10.671/46.531/120.371/7634.836 | from 1: 0.120/169.834/473.435/4905.500 (101); from 2: 0.055/187.595/1046.269/4904.349 (105) | 76/686/1283 |

### adacachy, over the band, run 4 (93e311f)

2026-11-02 09:19:02Z to 09:49:13Z, load 46.7-78.7, median 58.7 (sampled every 30 s). 7,191 writes acknowledged, 220
refused.

| member | write submit to durable p50/p99/p99.9/max ms | owner step gap p50/p99/p99.9/max ms | owner tick gap p50/p99/p99.9/max ms | answer gaps while leading p50/p99/p99.9/max ms (over 200 ms) | step-downs / elections / changes |
|---|---|---|---|---|---|
| 1 | 35.578/1514.219/4227.388/8323.568 | 1.074/2.204/42.030/5165.173 | 10.707/46.851/112.694/5167.328 | from 2: 0.122/161.406/967.338/5159.107 (91); from 3: 0.032/150.748/600.568/5158.490 (94) | 91/503/1020 |
| 2 | 35.359/1477.442/4366.502/8837.759 | 1.073/1.811/43.129/5165.593 | 10.706/47.846/117.666/5174.062 | from 1: 0.146/144.863/385.601/3869.546 (101); from 3: 0.071/155.182/555.999/3869.752 (105) | 91/503/1005 |
| 3 | 35.419/1493.028/4084.651/8323.507 | 1.073/1.798/43.248/5886.872 | 10.704/48.393/118.079/5892.227 | from 1: 0.148/142.651/337.546/5247.870 (88); from 2: 0.136/157.344/501.816/5246.961 (85) | 67/503/1006 |

### adacachy, loaded, run 5 (bc3b203)

2026-11-03 00:49:09Z to 01:19:22Z, load 25.4-125.4, median 40.3 (sampled every 30 s). 8,990 writes acknowledged,
188 refused.

| member | write submit to durable p50/p99/p99.9/max ms | owner step gap p50/p99/p99.9/max ms | owner tick gap p50/p99/p99.9/max ms | answer gaps while leading p50/p99/p99.9/max ms (over 200 ms) | step-downs / elections / changes |
|---|---|---|---|---|---|
| 1 | 36.143/550.960/3697.415/22200.168 | 1.062/2.273/46.566/8006.711 | 10.621/51.546/110.868/8012.019 | from 2: 0.089/172.452/375.240/3322.347 (150); from 3: 0.019/159.824/323.930/3319.998 (150) | 89/473/920 |
| 2 | 36.081/474.688/3492.732/22200.560 | 1.062/2.013/47.622/8006.008 | 10.619/52.352/110.750/8010.261 | from 1: 0.107/149.510/327.101/4000.720 (103); from 3: 0.048/161.719/557.682/4106.532 (104) | 63/473/900 |
| 3 | 36.110/518.109/3545.236/22200.197 | 1.061/1.972/47.402/8006.262 | 10.619/52.423/112.367/8009.437 | from 1: 0.117/137.725/281.362/4097.775 (84); from 2: 0.114/149.266/308.263/4096.841 (88) | 63/473/901 |

## Step 3: the quorum patience's tests, looped

Kiln goal 6b112f14, step 3 (replication.md, "The leader's quorum patience"). Each test's binary was run alone, 200 times,
by `disk-stalls-200.sh` (the probe carrier), with the load sampled before each run.

| host | commit (tree) | date | test | runs | failed | load |
|---|---|---|---|---|---|---|
| Adas-Mac-Studio-2 (macOS, M4 Max) | 220f47b (50190df) | 2026-11-06 16:25Z to 16:28Z | `a_leader_whose_followers_answer_as_late_as_it_measured_keeps_its_lead` | 200 | 0 | 4.54-6.25 |
| Adas-Mac-Studio-2 (macOS, M4 Max) | 220f47b (50190df) | 2026-11-06 16:25Z to 16:28Z | `a_leader_cut_off_from_its_followers_steps_down_within_its_patience` | 200 | 0 | 3.72-5.12 |

| Adas-Mac-Studio-3 (macOS, M4 Max), loaded as below | 7133cd8 (2afa2bf) | 2026-11-06 17:20Z to 17:48Z | `a_leader_whose_followers_answer_as_late_as_it_measured_keeps_its_lead` | 200 | 0 | 31.42-63.56, median 45.26 |
| Adas-Mac-Studio-3 (macOS, M4 Max), loaded as below | 7133cd8 (2afa2bf) | 2026-11-06 17:20Z to 17:48Z | `a_leader_cut_off_from_its_followers_steps_down_within_its_patience` | 200 | 0 | 26.56-38.60, median 28.66 |

Both nemesis tests passed after each loop: on Adas-Mac-Studio-2 at load 3.90 and 3.83, and on Adas-Mac-Studio-3
under the load at 26.42 and 26.44. Adas-Mac-Studio-2's runs are below the plan's load of 25 or more;
Adas-Mac-Studio-3's, made under the falsifier's load right after it, are all above it.

## The falsifier: the patience in force, under load

Kiln goal 6b112f14, the decision's falsifier (replication.md, "The leader's quorum patience"): step 1's measurement,
the same test, with the patience in force, on a host under load in step 1's band. It is falsified if a member's
quorum-check step-downs in a run are not fewer than the fewest of step 1's loaded runs at fixed ticks (63).

adacachy is ruled out (it runs production) and fedora did not answer, so the run is on Adas-Mac-Studio-3 (macOS, M4
Max, 14 cores, APFS on its internal SSD) under load the job made itself, holding the host's heavy lock
(`disk-stalls-loaded.sh`, probe carrier 272bfc2):
- disk and CPU together: six looping copies of the amphora test suite (14 threads each; its tests write databases)
  and a 1 GiB write-and-sync loop, always on;
- twelve groups of four CPU spinners, paused and resumed a group at a time to hold load1 within 25-55. macOS's load
  counts runnable threads only, not those waiting on the disk as Linux's does, so the disk load alone held it near 5;
- load1 and iostat sampled every 30 s. macOS's iostat gives no busy share; its transfers per second and MB/s stand
  for it.

25-55 on 14 cores is more per core than on adacachy's 32. The run is one, on another host than step 1's loaded runs;
a run at fixed ticks on the same host under the same load is its control.

### Adas-Mac-Studio-3, loaded, the patience in force (7133cd8)

2026-11-06 16:49:55Z to 17:20:10Z, load 6.09-54.96, median 49.13 (67 samples every 30 s, from the load's start; in
band 90 s before the run began at 50.57). Disk: 2143 to 21919 transfers a second. 4,441 writes acknowledged, 26
refused.

| member | write submit to durable p50/p99/p99.9/max ms | owner step gap p50/p99/p99.9/max ms | owner tick gap p50/p99/p99.9/max ms | answer gaps while leading p50/p99/p99.9/max ms (over 200 ms) | step-downs / elections / changes |
|---|---|---|---|---|---|
| 1 | 78.471/899.273/3590.718/7061.690 | 4.483/57.700/190.996/952.564 | 13.600/119.595/287.242/954.977 | from 2: 0.029/326.556/598.581/1374.210 (780); from 3: 0.025/300.762/542.861/1331.209 (756) | 12/170/249 |
| 2 | 77.622/1034.704/3910.353/9960.922 | 4.378/58.133/190.053/1049.374 | 13.635/118.835/278.470/1056.796 | from 1: 0.040/335.591/696.899/1813.522 (1161); from 3: 0.045/341.329/716.689/2793.207 (1118) | 11/169/240 |
| 3 | 78.039/996.562/3740.427/8180.929 | 4.891/63.646/198.244/3229.039 | 13.541/122.504/294.450/3233.587 | from 1: 0.046/291.208/616.215/1120.118 (136); from 2: 0.035/295.064/616.328/932.264 (125) | 8/171/272 |

Not falsified: the members' own quorum checks stepped them down 12, 11 and 8 times, each fewer than 63. The terms
still moved 169 to 171 times: most changes were a leader that stepped down or a follower that campaigned, which
the decision leaves to the followers' own timers (a member's own patience stays zero).
