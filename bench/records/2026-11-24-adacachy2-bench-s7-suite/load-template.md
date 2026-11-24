The window was to have no other swarm work on adacachy2. The machine ran other people's processes during the whole run; they were read-only to this lane and nothing was touched. `procs.txt` holds what was using the most CPU at each round's start.

- `kiln-5b8c64ebd8e73358 --test-threads=8`, the debug test binary of lane u2-kiln (PID 873176, nice 10): 6.4 cores (636-638% CPU) at every snapshot, from 05:56Z (its parent test run, PID 2377911, `cargo test --locked -p kiln --lib`, was seen at 06:03Z; the binary had run 10.7 hours by the smoke pass).
- `build_script_build` of kiln in the reach checkout `agy-int-f7b7b83737` (PID 2428752, nice 10): 1.1 to 1.7 cores in the smoke and round 1-3 snapshots.
- Slates lanes (through kiln's reach: `w-wall`, `w-ci-target`, `w-tsan*`): `cargo clippy --workspace`, `cargo test -p slates-server`, its rustc and rust-lld, at up to 2 cores each snapshot.
- kiln's `cargo check -p kiln --all-targets` (06:02Z), a kiln test server (PID 2777668, 0.3 cores), kiln's agents, Docker and a QEMU VM (under 0.1 cores).

Load, CPU and I/O pressure per phase (from `phases.log`, 5 s samples; the load average counts this run's own threads too):

TODO-PHASES
