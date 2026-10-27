/**
 * Whether release `a` is older than `b`, in semver order as the node's updater has it: by
 * major.minor.patch, then a prerelease is older than its release, and prereleases compare
 * part by part (`beta.2` < `beta.10` < `rc.1`).
 */
export function olderThan(a: string, b: string): boolean {
  const parse = (v: string) => {
    const [main = '', ...rest] = v.split('+', 1)[0]!.split('-')
    const pre = rest.join('-')
    return { parts: main.split('.').map((p) => Number(p) || 0), pre: pre ? pre.split('.') : [] }
  }
  const x = parse(a)
  const y = parse(b)
  for (let i = 0; i < 3; i++) {
    const d = (x.parts[i] ?? 0) - (y.parts[i] ?? 0)
    if (d) return d < 0
  }
  if (!x.pre.length || !y.pre.length) return x.pre.length > 0 && y.pre.length === 0
  for (let i = 0; i < Math.max(x.pre.length, y.pre.length); i++) {
    const p = x.pre[i]
    const q = y.pre[i]
    // A shorter list that matches so far is older (`beta` < `beta.1`).
    if (p === undefined || q === undefined) return p === undefined
    if (p === q) continue
    const numeric = /^\d+$/
    if (numeric.test(p) && numeric.test(q)) return Number(p) < Number(q)
    // Numbers sort before words.
    if (numeric.test(p) !== numeric.test(q)) return numeric.test(p)
    return p < q
  }
  return false
}
