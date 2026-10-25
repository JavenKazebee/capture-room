/**
 * Whether release `a` is older than `b`, by major.minor.patch. A prerelease counts as older
 * than its release, as the node's updater (semver) has it.
 */
export function olderThan(a: string, b: string): boolean {
  const parse = (v: string) => {
    const [core = '', pre] = v.split('-', 2)
    return { parts: core.split('.').map((p) => Number(p) || 0), pre }
  }
  const x = parse(a)
  const y = parse(b)
  for (let i = 0; i < 3; i++) {
    const d = (x.parts[i] ?? 0) - (y.parts[i] ?? 0)
    if (d) return d < 0
  }
  return x.pre !== undefined && (y.pre === undefined || x.pre < y.pre)
}
