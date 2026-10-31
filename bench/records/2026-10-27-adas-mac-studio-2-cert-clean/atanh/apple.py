# Is this platform's atanh 0.5*log1p(2a/(1-a)) on a = |x|, x itself below a threshold?
import math, random, sys, collections
random.seed(7)
def A(x, th):
    a = abs(x)
    if a < th: return x
    return math.copysign(0.5 * math.log1p(2 * a / (1 - a)), x)
xs = [random.uniform(-1, 1) for _ in range(200000)]
xs += [random.choice((-1, 1)) * random.uniform(0.5, 1) * 2.0 ** -random.randint(1, 60) for _ in range(200000)]
xs += [random.choice((-1, 1)) * (1 - 2.0 ** -random.uniform(1, 52)) for _ in range(50000)]
xs = [x for x in xs if -1 < x < 1]
for th in (0.0, 2.0 ** -26, 2.0 ** -27, 2.0 ** -28, 2.0 ** -29):
    bad = [x for x in xs if math.atanh(x) != A(x, th)]
    hist = collections.Counter(math.frexp(abs(x))[1] for x in bad)
    print(f"threshold 2^{math.frexp(th)[1] - 1 if th else '-inf'}: {len(bad)} of {len(xs)} differ; by exponent {sorted(hist.items())[:12]}")
    if len(bad) and th == 2.0 ** -28:
        for x in bad[:6]: print("   ", repr(x), repr(math.atanh(x)), repr(A(x, th)))
