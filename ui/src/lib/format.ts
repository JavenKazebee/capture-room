const pad = (n: number) => String(n).padStart(2, '0')

/** Elapsed time as `MM:SS`, or `HH:MM:SS` from an hour on. */
export function formatDuration(ms: number): string {
  const elapsed = Math.max(0, Math.floor(ms / 1000))
  const h = Math.floor(elapsed / 3600)
  const m = Math.floor((elapsed % 3600) / 60)
  const s = elapsed % 60
  return h > 0 ? `${pad(h)}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`
}

/** Uptime as `3h 12m` or `12m`. */
export function formatUptime(secs: number): string {
  const h = Math.floor(secs / 3600)
  const m = Math.floor((secs % 3600) / 60)
  return h > 0 ? `${h}h ${m}m` : `${m}m`
}

/** Decimal byte size: `512 B`, `1.5 GB`, `120 GB`. */
export function formatBytes(n: number): string {
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let i = 0
  while (n >= 1000 && i < units.length - 1) {
    n /= 1000
    i++
  }
  return `${n.toFixed(n >= 100 || i === 0 ? 0 : 1)} ${units[i]}`
}
