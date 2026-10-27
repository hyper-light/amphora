# atanh of what the generator makes: its numeric literals and one +, -, *, / of two, in (-1, 1).
import math, sys, itertools
from decimal import Decimal, getcontext
getcontext().prec = 60
def ref(x):
    d = Decimal(x)
    if abs(x) < 1e-4:
        # 60 digits can't hold 1 + 2x here: the series, whose 12th term is below 1e-100 of x.
        return float(sum(d ** (2 * k + 1) / (2 * k + 1) for k in range(12)))
    return float(((1 + d) / (1 - d)).ln() / 2)
def A(x):
    a = abs(x)
    return math.copysign(0.5 * math.log1p(2 * a / (1 - a)), x)
lits = [0, 1, -1, 2, 3, 7, 10, 100, -100, 255, 2**63 - 1, -2**63, 9007199254740993, 0.0, 0.5, 1.5, -2.5, 3.0, 1e15, 1e308, -1e-308, 2.0, 0.1, 1.5, 10, -1, 0, 1, 1e3, 16]
vals = set()
for a in lits:
    vals.add(float(a)); vals.add(-float(a))
for a, b in itertools.product(lits, repeat=2):
    fa, fb = float(a), float(b)
    for v in (fa + fb, fa - fb, fa * fb):
        vals.add(v)
    if fb != 0 and not (isinstance(a, int) and isinstance(b, int)):
        vals.add(fa / fb)
xs = sorted(v for v in vals if -1 < v < 1 and v == v)
cr = a_ = 0
for x in xs:
    lib, r, f = math.atanh(x), ref(x), A(x)
    cr += lib == r; a_ += lib == f
    if lib != r or lib != f:
        print(f"{x!r:>24} libc {lib!r:>24} cr {r!r:>24} A {f!r:>24}")
print(sys.platform, "values", len(xs), "libc == correctly rounded", cr, "libc == A", a_)
