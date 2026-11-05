// Small statistics for the scenarios (net-test 4.4): quantiles of a sample, a summary line, the slope of a
// series (for the leak detector of `soak`). Plain numbers in, plain numbers out.

/** The `p`-quantile (0..1) of `xs`, nearest-rank; `NaN` for an empty sample. */
export function quantile(xs, p) {
  if (!xs.length) return NaN;
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.max(0, Math.ceil(p * s.length) - 1))];
}

export const median = (xs) => quantile(xs, 0.5);

export const round = (x, digits = 1) => (Number.isFinite(x) ? Math.round(x * 10 ** digits) / 10 ** digits : null);

/** `{n, min, p50, p95, p99, max, mean}` of a sample (milliseconds, say), rounded to a tenth. */
export function summary(xs) {
  const v = xs.filter((x) => Number.isFinite(x));
  if (!v.length) return { n: 0, min: null, p50: null, p95: null, p99: null, max: null, mean: null };
  return {
    n: v.length,
    min: round(Math.min(...v)),
    p50: round(quantile(v, 0.5)),
    p95: round(quantile(v, 0.95)),
    p99: round(quantile(v, 0.99)),
    max: round(Math.max(...v)),
    mean: round(v.reduce((s, x) => s + x, 0) / v.length),
  };
}

/** The least-squares slope of `ys` against `xs` (units of y per unit of x); `0` for fewer than two points. */
export function slope(xs, ys) {
  const n = Math.min(xs.length, ys.length);
  if (n < 2) return 0;
  const mx = xs.slice(0, n).reduce((s, x) => s + x, 0) / n;
  const my = ys.slice(0, n).reduce((s, y) => s + y, 0) / n;
  let num = 0;
  let den = 0;
  for (let i = 0; i < n; i++) {
    num += (xs[i] - mx) * (ys[i] - my);
    den += (xs[i] - mx) ** 2;
  }
  return den === 0 ? 0 : num / den;
}

/** A seeded generator (mulberry32) so that a chaos run can be repeated with `--seed`. */
export function rng(seed) {
  let a = (seed >>> 0) || 1;
  const next = () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  return {
    next,
    int: (lo, hi) => lo + Math.floor(next() * (hi - lo + 1)),
    pick: (xs) => xs[Math.floor(next() * xs.length)],
  };
}
