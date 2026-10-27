# Which formula over this platform's log1p/log gives its C library's atanh, bit for bit?
import math, random, sys, struct
from decimal import Decimal, getcontext
getcontext().prec = 60
random.seed(5)
def ref(x):
    d = Decimal(x)
    if abs(x) < 1e-4:
        # 60 digits can't hold 1 + 2x here: the series, whose 12th term is below 1e-100 of x.
        return float(sum(d ** (2 * k + 1) / (2 * k + 1) for k in range(12)))
    return float(((1 + d) / (1 - d)).ln() / 2)
def A(a): return 0.5 * math.log1p(2 * a / (1 - a))
def B(a):
    if a < 0.5:
        t = a + a
        return 0.5 * math.log1p(t + t * a / (1 - a))
    return 0.5 * math.log1p((a + a) / (1 - a))
def C(a): return 0.5 * math.log((1 + a) / (1 - a))
def D(a): return 0.5 * (math.log1p(a) - math.log1p(-a))
forms = dict(A=A, B=B, C=C, D=D)
xs = [random.uniform(0, 1) for _ in range(40000)] + [random.uniform(0, 0.5) * 2.0 ** -random.randint(0, 30) for _ in range(20000)] + [1 - 2.0 ** -random.uniform(1, 52) for _ in range(10000)]
xs = [x for x in xs if 0 < x < 1]
n = len(xs)
same = {k: 0 for k in forms}; refsame = 0; refform = {k: 0 for k in forms}
for x in xs:
    lib = math.atanh(x); r = ref(x) if len(sys.argv) > 1 else None
    for k, f in forms.items():
        v = f(x)
        if v == lib: same[k] += 1
        if r is not None and v == r: refform[k] += 1
    if r is not None and r == lib: refsame += 1
print(sys.platform, "n", n, "formula == libc:", {k: f"{v/n:.4f}" for k, v in same.items()})
if len(sys.argv) > 1: print("libc == ref", f"{refsame/n:.4f}", "formula == ref:", {k: f"{v/n:.4f}" for k, v in refform.items()})
