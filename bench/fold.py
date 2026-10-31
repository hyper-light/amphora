# Tallies a profile's folded stacks (AMPHORA_PROFILE=<prefix> writes <prefix>.folded): the frames
# under one frame, by self and by total, for one thread.
# usage: python3 fold.py <prefix>.folded <thread> [frame under which to tally] [rows]
import sys, collections

path, thread = sys.argv[1], sys.argv[2]
under = sys.argv[3] if len(sys.argv) > 3 else None
rows = int(sys.argv[4]) if len(sys.argv) > 4 else 60
own, total, children, n = collections.Counter(), collections.Counter(), collections.Counter(), 0
for line in open(path):
    stack, _, count = line.rstrip("\n").rpartition(" ")
    names = stack.split(";")
    if names[0] != thread:
        continue
    names, count = names[1:], int(count)
    if under is not None:
        if under not in names:
            continue
        i = names.index(under)
        names = names[i:]
        if len(names) > 1:
            children[names[1]] += count
    n += count
    own[names[-1]] += count
    for name in set(names):
        total[name] += count
print(f"samples {n}" + (f" under {under}" if under else ""))
if under:
    print("\n== called directly ==")
    for k, c in children.most_common(rows):
        print(f"{100*c/n:6.2f}% {k}")
print("\n== self ==")
for k, c in own.most_common(rows):
    print(f"{100*c/n:6.2f}% {k}")
print("\n== total ==")
for k, c in total.most_common(rows):
    print(f"{100*c/n:6.2f}% {k}")
