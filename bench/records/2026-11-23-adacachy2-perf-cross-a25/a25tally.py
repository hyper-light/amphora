#!/usr/bin/env python3
# Lane A25: counts over the logs a24suite.sh wrote. Per arm: the runs, r5's result in each (a log with no r5 line is
# named), each test's failures, and what each failed r5 printed. usage: a25tally.py OUT SHA_A SHA_B [LABEL_PREFIX]
import collections, glob, os, re, sys

out, shas = sys.argv[1], sys.argv[2:4]
prefix = sys.argv[4] if len(sys.argv) > 4 else ""
R5 = "session_reads::t_t1_10_r5_a_session_reads_its_write_on_a_lagging_follower"
print("out", out)
table = {}
for sha in shas:
    logs = sorted(glob.glob(os.path.join(out, prefix + "*-" + sha + ".log")))
    fails, r5, no_r5, short, seen = collections.Counter(), collections.Counter(), [], [], 0
    for log in logs:
        text = open(log, errors="replace").read()
        name = os.path.basename(log).split("-" + sha)[0]
        if not re.search(r"^test result: ", text, re.M):
            short.append(name)
        seen += 1
        m = re.search(r"^test \S*" + re.escape(R5) + r" \.\.\. (\w+)$", text, re.M)
        if m is None:
            no_r5.append(name)
        else:
            r5[m.group(1)] += 1
        failed = re.findall(r"^test (\S+) \.\.\. FAILED$", text, re.M)
        for t in failed:
            fails[t.replace("database::", "", 1)] += 1
        if m is not None and m.group(1) == "FAILED":
            block = re.search(r"---- \S*" + re.escape(R5) + r" stdout ----\n(.*?)(?=\n---- |\nsuccesses:|\nfailures:|\Z)", text, re.S)
            print("  r5 failed in", name, "sha", sha[:8])
            for line in (block.group(1) if block else "").splitlines():
                if re.search(r"panicked at|assertion|left:|right:|SessionCounters|waited", line):
                    print("     ", line.strip()[:260])
    print(f"{sha[:8]}: {seen} runs; r5 {dict(r5)}; no r5 line: {no_r5}; no 'test result' line: {short}")
    for t, n in fails.most_common():
        print(f"     {n:3d}  {t}")
    table[sha] = (seen, r5)
a, b = (table[s] for s in shas)
print("a24suite rounds a side:", a[0], b[0])
