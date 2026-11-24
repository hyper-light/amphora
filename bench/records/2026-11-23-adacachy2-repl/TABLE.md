## bench/records/2026-11-23-adacachy2-repl

Median (min-max, n runs). Latencies in ms. column = what an acknowledgement means; quorum and primary-only rows are not comparable.

| record | column | writers/behind | samples/run (min) | p50 | p99 | p99.9 | commits/s | refused | other numbers (median) |
|---|---|---|---|---|---|---|---|---|---|
| libsql-commit |  |  | 0 |  |  |  |  | unavailable | cpus=32, nodes=3, unix_s=1.79544e+09 |
| litefs-commit |  |  | 0 |  |  |  |  | unavailable | cpus=32, nodes=3, unix_s=1.79544e+09 |
| litefs-commit | primary only | 1 | 74 | 64.000 (30.976-64.512, n=3) | 92.160 (39.936-163.840, n=3) | withheld (min samples 74 < 1000) | 15.400 (14.800-31.000, n=3) |  | cpus=32, events_per_s=15.4, nodes=3, unix_s=1.79544e+09 |
| amphora-procs-persisted-commit | quorum | 1 | 98 | 47.616 (39.424-48.640, n=5) | 83.968 (71.680-107.520, n=5) | withheld (min samples 98 < 1000) | 19.800 (19.600-23.200, n=5) |  | cpus=32, events_per_s=19.8, nodes=3, unix_s=1.79544e+09 |
| amphora-procs-persisted-commit | quorum | 32 | 61 | 1638.400 (1474.560-1998.848, n=5) | 5832.704 (5308.416-6488.064, n=5) | withheld (min samples 61 < 1000) | 13.800 (12.200-16.000, n=5) |  | cpus=32, events_per_s=13.8, nodes=3, unix_s=1.79544e+09 |
| amphora-procs-replicated-commit | quorum | 1 | 65 | 69.632 (56.320-75.776, n=5) | 94.208 (73.728-147.456, n=5) | withheld (min samples 65 < 1000) | 14.400 (13.000-17.000, n=5) |  | cpus=32, events_per_s=14.4, nodes=3, unix_s=1.79544e+09 |
| amphora-procs-replicated-commit | quorum | 32 | 90 | 991.232 (974.848-1196.032, n=5) | 4784.128 (3309.568-6160.384, n=5) | withheld (min samples 90 < 1000) | 19.000 (18.000-21.600, n=5) |  | cpus=32, events_per_s=19, nodes=3, unix_s=1.79544e+09 |
| amphora-procs-replicated-commit | quorum | 128 | 85 | 7274.496 (6750.208-7798.784, n=2) | 11796.480 (11796.480-11796.480, n=2) | withheld (min samples 85 < 1000) | 17.300 (17.000-17.600, n=2) |  | cpus=32, events_per_s=17.3, nodes=3, unix_s=1.79544e+09 |
| dqlite-commit | quorum | 1 | 657 | 6.336 (5.888-7.936, n=5) | 9.472 (8.448-10.496, n=5) | withheld (min samples 657 < 1000) | 145.400 (131.400-174.000, n=5) |  | cpus=32, events_per_s=145.4, nodes=3, unix_s=1.79544e+09 |
| dqlite-commit | quorum | 32 | 648 | 141.312 (106.496-172.032, n=5) | 925.696 (663.552-1064.960, n=5) | withheld (min samples 648 < 1000) | 153.200 (129.600-196.400, n=5) |  | cpus=32, events_per_s=153.2, nodes=3, unix_s=1.79544e+09 |
| dqlite-commit | quorum | 128 | 674 | 503.808 (372.736-647.168, n=5) | 2686.976 (2162.688-3407.872, n=5) | withheld (min samples 674 < 1000) | 187.800 (134.800-234.800, n=5) |  | cpus=32, events_per_s=187.8, nodes=3, unix_s=1.79544e+09 |
| rqlite-commit | quorum | 1 | 119 | 40.448 (36.352-41.472, n=5) | 60.416 (54.272-65.024, n=5) | withheld (min samples 119 < 1000) | 24.600 (23.800-27.400, n=5) |  | cpus=32, events_per_s=24.6, nodes=3, unix_s=1.79544e+09 |
| rqlite-commit | quorum | 32 | 1796 | 83.968 (78.848-88.064, n=5) | 130.048 (104.448-143.360, n=5) | 131.072 (109.568-163.840, n=5) | 372.800 (359.200-397.800, n=5) |  | cpus=32, events_per_s=372.8, nodes=3, unix_s=1.79544e+09 |
| rqlite-commit | quorum | 128 | 5973 | 89.088 (80.896-97.280, n=5) | 133.120 (124.928-434.176, n=5) | 167.936 (135.168-434.176, n=5) | 1389.800 (1194.600-1478.600, n=5) |  | cpus=32, events_per_s=1389.8, nodes=3, unix_s=1.79544e+09 |


```
round 1 start (amphora-procs-persisted amphora-procs-replicated rqlite dqlite libsql litefs) 2026-11-23T12:57:06Z load 14.42 19.38 26.74 | cpu some avg10=0.38 avg60=0.18 avg300=1.71 total=8136792457 | io some avg10=2.85 avg60=3.75 avg300=3.53 total=17334380491 / full avg10=2.46 avg60=3.20 avg300=2.82 total=15528563614
round 1 exit 1
round 1 end 2026-11-23T12:59:09Z load 19.69 19.08 25.70 | cpu some avg10=1.96 avg60=0.93 avg300=1.37 total=8138304478 | io some avg10=3.38 avg60=3.47 avg300=3.49 total=17339166034 / full avg10=2.30 avg60=2.67 avg300=2.77 total=15532499165
round 2 start (litefs libsql dqlite rqlite amphora-procs-replicated amphora-procs-persisted) 2026-11-23T12:59:09Z load 19.69 19.08 25.70 | cpu some avg10=1.96 avg60=0.93 avg300=1.37 total=8138304479 | io some avg10=3.38 avg60=3.47 avg300=3.49 total=17339166193 / full avg10=2.30 avg60=2.67 avg300=2.77 total=15532499322
round 2 exit 1
round 2 end 2026-11-23T13:01:08Z load 21.84 19.55 25.02 | cpu some avg10=1.02 avg60=1.21 avg300=1.28 total=8140244734 | io some avg10=3.75 avg60=3.66 avg300=3.57 total=17344101757 / full avg10=2.63 avg60=2.60 avg300=2.75 total=15536395967
round 3 start (amphora-procs-persisted amphora-procs-replicated rqlite dqlite libsql litefs) 2026-11-23T13:01:08Z load 21.84 19.55 25.02 | cpu some avg10=1.02 avg60=1.21 avg300=1.28 total=8140244750 | io some avg10=3.75 avg60=3.66 avg300=3.57 total=17344101988 / full avg10=2.63 avg60=2.60 avg300=2.75 total=15536396089
round 3 exit 1
round 3 end 2026-11-23T13:03:21Z load 18.22 19.30 24.20 | cpu some avg10=2.77 avg60=1.42 avg300=1.18 total=8142376084 | io some avg10=3.58 avg60=4.04 avg300=3.84 total=17350515683 / full avg10=2.27 avg60=2.96 avg300=2.95 total=15541530832
round 4 start (litefs libsql dqlite rqlite amphora-procs-replicated amphora-procs-persisted) 2026-11-23T13:03:21Z load 18.22 19.30 24.20 | cpu some avg10=2.77 avg60=1.42 avg300=1.18 total=8142376143 | io some avg10=3.58 avg60=4.04 avg300=3.84 total=17350515774 / full avg10=2.27 avg60=2.96 avg300=2.95 total=15541530851
round 4 exit 1
round 4 end 2026-11-23T13:05:20Z load 21.07 19.42 23.61 | cpu some avg10=1.51 avg60=1.45 avg300=1.19 total=8144464356 | io some avg10=3.65 avg60=3.80 avg300=3.84 total=17355655467 / full avg10=2.29 avg60=2.72 avg300=2.91 total=15545593757
round 5 start (amphora-procs-persisted amphora-procs-replicated rqlite dqlite libsql litefs) 2026-11-23T13:05:20Z load 21.07 19.42 23.61 | cpu some avg10=1.51 avg60=1.45 avg300=1.19 total=8144464356 | io some avg10=3.65 avg60=3.80 avg300=3.84 total=17355655467 / full avg10=2.29 avg60=2.72 avg300=2.91 total=15545593757
round 5 exit 1
round 5 end 2026-11-23T13:07:23Z load 23.22 20.55 23.50 | cpu some avg10=3.58 avg60=1.87 avg300=1.29 total=8146811014 | io some avg10=2.07 avg60=3.71 avg300=3.84 total=17360962133 / full avg10=1.44 avg60=2.63 avg300=2.86 total=15549665422
```
