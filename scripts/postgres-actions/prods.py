"""Each nonterminal's productions in gram.y, with line numbers.

python3 scripts/postgres-actions/prods.py <repo> <nonterminal>...
"""
import re, sys
g = open(sys.argv[1] + "/vendor/postgres/src/backend/parser/gram.y").read().split("\n")
for name in sys.argv[2:]:
    for i, l in enumerate(g):
        if re.match(re.escape(name) + r"\s*:", l):
            j = i
            while j < len(g) and not re.match(r"^\t*;\s*$", g[j]):
                j += 1
            print("\n".join(f"{k+1}: {g[k]}" for k in range(i, j + 1)))
            break
