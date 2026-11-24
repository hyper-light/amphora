"""Run the verifier's paired release binaries on Linux. See the adjacent benchmark record."""

import argparse
import datetime as dt
import hashlib
import json
import math
import os
from pathlib import Path
import re
import subprocess
import time


def trials(smoke):
    """First cover the historical workloads. Run the optional checkout comparison last."""
    ladder = "1,4,16,64,256,1024" if not smoke else "1,2"
    plain_clients = (1, 8, 32, 64, 128) if not smoke else (1,)
    point_clients = (1, 64) if not smoke else (1,)
    # The historical CPU-cache experiment starts 25 fresh processes in each arm.
    cpu_rounds = (8, 8, 9) if not smoke else (1,)
    cpu_rep = 0
    cold_rep = 0
    for rep, cpu_count in enumerate(cpu_rounds, 1):
        for _ in range(cpu_count):
            cpu_rep += 1
            for arm, binary in (("before", "cpu-base-open_time"), ("after", "cpu-after-open_time")):
                yield ("cpu", arm, cpu_rep, binary, ["$fixture", "1"], {})
        for _ in range((2, 2, 1)[rep - 1] if not smoke else 1):
            cold_rep += 1
            for arm, binary in (("before", "base-open_time"), ("after", "cold-after-open_time")):
                yield ("cold", arm, cold_rep, binary, ["$fixture", "200" if not smoke else "2"], {})
        for clients in point_clients:
            each = 4000 // clients + 200 if not smoke else 3
            for storage in ("versioned", "default"):
                args = [storage, "$fixture", "10000" if not smoke else "17", str(clients), str(each)]
                for arm, binary in (("before", "base-versioned_point_reads"), ("after", "point-after-versioned_point_reads")):
                    yield ("point", arm, rep, binary, args, {})
        for op in ("point_pk", "point_secondary"):
            for clients in plain_clients:
                for arm in ("before", "after"):
                    args = ["--suite", "--target", "amphora", "--dir", "$plain", "--reuse",
                            "--only", op + "$", "--clients", str(clients), "--seconds", "5" if not smoke else "1",
                            "--warmup", "1", "--items", "200000"]
                    yield ("plain", arm, rep, "plain-" + arm, args, {"AMPHORA_PAGE_PATH": "plain"})
        for arm, binary in (("before", "base-admission"), ("after", "memory-after-admission")):
            args = ["$fixture", "200000" if not smoke else "2", "3" if not smoke else "0.1", ladder, "call"]
            yield ("admission", arm, rep, binary, args, {})
    for rep in range(1, 4 if not smoke else 2):
        for clients in (32, 1024) if not smoke else (2,):
            for arm in ("pool", "slots"):
                args = ["--suite", "--target", "amphora", "--dir", "$plain", "--reuse",
                        "--only", "point_pk$", "--clients", str(clients),
                        "--seconds", "5" if not smoke else "1", "--warmup", "1", "--items", "200000"]
                yield ("checkout", arm, rep, "checkout", args, {"AMPHORA_READ_CHECKOUT": arm})


def observation():
    pressure = {}
    for resource in ("cpu", "io"):
        for line in Path("/proc/pressure", resource).read_text().splitlines():
            kind, *fields = line.split()
            values = dict(field.split("=") for field in fields)
            pressure[f"{resource}_{kind}"] = (float(values["avg10"]), int(values["total"]))
    return time.monotonic(), os.getloadavg()[0], pressure


def host_metrics(samples):
    start, first, pressure = samples[0]
    end, last, final = samples[-1]
    peak = max(sample[1] for sample in samples)
    result = {"samples": len(samples), "load1_start": first, "load1_end": last,
              "load1_max": peak, "load_ge_2": peak >= 2, "psi": {}}
    for key, (avg, total) in pressure.items():
        stall_us = final[key][1] - total
        result["psi"][key] = {"avg10_start": avg, "avg10_end": final[key][0],
                              "avg10_max": max(s[2][key][0] for s in samples),
                              "stall_us": stall_us, "stall_pct": round(stall_us / ((end - start) * 10000), 4)}
    return result


def require(ok, message):
    if not ok:
        raise ValueError(message)


def measurements(family, args, raw):
    if family == "point":
        vals = dict(word.split("=") for word in raw.split())
        reads = int(vals["reads"])
        require((vals["storage"], vals["rows"], vals["clients"]) == (args[0], args[2], args[3]), "wrong point-read fixture")
        require(reads == int(args[3]) * int(args[4]), "missing verified point reads")
        return {"verified_values": reads, "reads_per_s": float(vals["reads_per_s"]),
                "seconds": float(vals["seconds"])}
    if family in ("cold", "cpu"):
        first = re.search(r"process's first open: ([\d.]+)us", raw)
        require(first, "missing first open")
        phases = {m[1]: {"n": int(m[2]), "min_us": float(m[3]), "median_us": float(m[4]), "max_us": float(m[5])}
                  for m in re.finditer(r"^(.+): n=(\d+) min=([\d.]+)us median=([\d.]+)us max=([\d.]+)us$", raw, re.M)}
        require(set(phases) == {"new open", "reopen open", "new first statement", "reopen first statement"} and
                all(p["n"] == int(args[1]) for p in phases.values()), "incomplete open phases")
        return {"first_open_us": float(first[1]), "phases": phases}
    if family == "admission":
        points = [json.loads(line) for line in raw.splitlines() if line.startswith("{")]
        room, *calls = points
        require(room["what"] == "room" and room["ns_per_call"] > 0, "missing room measurement")
        require([c["clients"] for c in calls] == list(map(int, args[3].split(","))), "incomplete admission ladder")
        require(all(c["what"] == "call" and c["completed"] + c["refused"] > 0 for c in calls), "empty admission cell")
        return {"room": room, "cells": calls}
    report = json.loads(raw)
    require(report["reused"] and report["target"] == "amphora" and report["items"] == 200000, "wrong suite fixture")
    clients = list(map(int, args[args.index("--clients") + 1].split(",")))
    ops = args[args.index("--only") + 1].replace("$", "").split(",")
    cells = []
    for cell in report["results"]:
        stats = cell["ms"] or {}
        cells.append({k: cell[k] for k in ("op", "clients", "ops_per_s", "rows_per_op", "errors", "retries", "error")} |
                     {"n": stats.get("n", 0), "p50_ms": stats.get("p50"),
                      "p99_ms": stats.get("p99"), "p999_ms": stats.get("p999")})
    require([(c["op"], c["clients"]) for c in cells] == [(op, n) for op in ops for n in clients], "incomplete suite ladder")
    return {"cells": cells}


def iso(unix):
    return dt.datetime.fromtimestamp(unix, dt.timezone.utc).isoformat(timespec="milliseconds")


def settle(available):
    """Allow the one-minute host load to decay after a build or a high-concurrency arm."""
    first = last = os.getloadavg()[0]
    started = time.monotonic()
    # Linux kernel/sched/loadavg.c decays load over one minute, updating about every five
    # seconds. This allows the observed load to reach two on an idle host, plus one update.
    allowance = min(available, 60 * math.log(max(first, 2) / 2) + 5)
    while last >= 2 and (left := allowance - (time.monotonic() - started)) > 0:
        time.sleep(min(1, left))
        last = os.getloadavg()[0]
    return {"load1_first": first, "load1_last": last, "wait_s": round(time.monotonic() - started, 3)}


def run(args):
    lab = args.lab.resolve()
    output = lab / args.output
    output.mkdir()
    cache = output / "cache"
    cache.mkdir()
    planned = list(trials(args.smoke))
    if args.only:
        planned = [p for p in planned if p[0] in args.only]
    start = dt.datetime.fromisoformat(args.start).timestamp() if args.start else time.time()
    end = dt.datetime.fromisoformat(args.end).timestamp() if args.end else float("inf")
    names = sorted({p[3] for p in planned})
    while time.time() < start and any(not (lab / "bin" / name).is_file() for name in names):
        time.sleep(min(1, max(0, start - time.time())))
    hashes = {}
    for name in names:
        executable = lab / "bin" / name
        if executable.is_file():
            with executable.open("rb") as binary:
                hashes[name] = hashlib.file_digest(binary, "sha256").hexdigest()
    while (remaining := start - time.time()) > 0:
        time.sleep(min(1, remaining))
    if args.start and args.end:
        (output / "opening-load.json").write_text(json.dumps(settle(max(0, end - time.time()))) + "\n")
    suite_times = []
    with (output / "runs.jsonl").open("x", buffering=1) as records:
        for number, (family, arm, rep, name, argv, flags) in enumerate(planned, 1):
            record = {"number": number, "family": family, "arm": arm, "rep": rep, "binary": name,
                      "sha256": hashes.get(name), "args": argv, "env": flags, "started_utc": iso(time.time())}
            if family == "checkout" and args.end and name in hashes and time.time() < end:
                # Reserve the longest observed suite time for every remaining arm. Use
                # its share of the spare time to let the previous run's load decay.
                remaining_runs = len(planned) - number + 1
                measured = int(argv[argv.index("--seconds") + 1]) + int(argv[argv.index("--warmup") + 1])
                reserve = max([measured, *suite_times])
                spare = (end - time.time()) / remaining_runs - reserve
                record["settle"] = settle(max(0, spare))
            if time.time() >= end or name not in hashes:
                record["not_started"] = "window ended" if time.time() >= end else "binary unavailable"
            else:
                record["started_utc"] = iso(time.time())
                fixture = output / f"fixture-{number}"
                paths = {"$fixture": str(fixture), "$plain": str(lab / "data/plain")}
                cmd = [str(lab / "bin" / name)] + [paths.get(a, a) for a in argv]
                # The CPU cache arm has one private, initially empty cache shared by its processes.
                env = {k: v for k, v in os.environ.items() if not k.startswith("AMPHORA_")}
                env.update(flags, XDG_CACHE_HOME=str(cache / family))
                stdout = output / f"{number:03d}.out"
                stderr = output / f"{number:03d}.err"
                samples = [observation()]
                with stdout.open("xb") as out, stderr.open("xb") as err:
                    try:
                        proc = subprocess.Popen(cmd, stdout=out, stderr=err, env=env)
                    except OSError as error:
                        proc = None
                        record["launch_error"] = str(error)
                    while proc is not None and proc.poll() is None:
                        left = end - time.time()
                        if left <= 0:
                            record["window_cutoff"] = True
                            proc.kill()
                            proc.wait()
                            break
                        try:
                            proc.wait(timeout=min(left, 1))
                        except subprocess.TimeoutExpired:
                            samples.append(observation())
                samples.append(observation())
                status = proc.returncode if proc is not None else None
                record.update(host_metrics(samples), status=status, finished_utc=iso(time.time()),
                              elapsed_s=round(samples[-1][0] - samples[0][0], 4))
                if family in ("plain", "checkout"):
                    suite_times.append(record["elapsed_s"])
                try:
                    record["metrics"] = measurements(family, argv, stdout.read_text())
                    cells = record["metrics"].get("cells", [])
                    if family == "admission":
                        require(all(c["unexpected_errors"] == 0 for c in cells), "unexpected admission errors")
                    elif cells:
                        require(all(c["errors"] == 0 and c["error"] is None and c["n"] > 0 and c["rows_per_op"] == 1 for c in cells), "suite errors or wrong row counts")
                        require(all(0 <= c["p50_ms"] <= c["p99_ms"] <= c["p999_ms"] < math.inf for c in cells), "invalid latency percentiles")
                except (IndexError, KeyError, TypeError, ValueError) as error:
                    record["invalid"] = str(error)
                if record.get("invalid") or status != 0:
                    record["partial_stdout"] = stdout.read_text(errors="replace")[-4000:]
                    record["stderr_tail"] = stderr.read_text(errors="replace")[-1500:]
            records.write(json.dumps(record, separators=(",", ":")) + "\n")
            print(number, family, arm, rep, record.get("status", record.get("not_started")),
                  record.get("load1_max"), record.get("invalid", ""), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("lab", type=Path)
    parser.add_argument("output", help="new directory in lab for this run")
    parser.add_argument("--start", help="ISO 8601 timestamp with timezone")
    parser.add_argument("--end", help="ISO 8601 timestamp with timezone")
    parser.add_argument("--only", nargs="+", choices=("point", "cold", "cpu", "plain", "admission", "checkout"))
    parser.add_argument("--smoke", action="store_true")
    run(parser.parse_args())
