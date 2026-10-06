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

/** A data rate: `1.4 MB/s`. */
export function formatRate(bytesPerSec: number): string {
  return `${formatBytes(bytesPerSec)}/s`
}

/** Recording time left, coarse: `2d 4h`, `3h 20m`, `45m`, `<1m`. */
export function formatTimeLeft(secs: number): string {
  const d = Math.floor(secs / 86400)
  const h = Math.floor((secs % 86400) / 3600)
  const m = Math.floor((secs % 3600) / 60)
  if (d > 0) return `${d}d ${h}h`
  if (h > 0) return `${h}h ${m}m`
  return m > 0 ? `${m}m` : '<1m'
}

/** A wall-clock time as `13:10:22`. */
export const clockTime = (d: Date | string) => new Date(d).toLocaleTimeString([], { hour12: false, hour: '2-digit', minute: '2-digit', second: '2-digit' })

/**
 * When something ran, as `13:10:22 → 13:55:24 · 45:02`, or `14:02:10 → now · 12:34`
 * while it's still going. `withDate` prefixes the start's date (`Oct 5 13:10:22 → …`).
 */
export function formatTimeRange(start: string, end: string | null, now: Date, opts: { withDate?: boolean } = {}): string {
  const from = new Date(start)
  const to = end ? new Date(end) : null
  const date = opts.withDate ? `${from.toLocaleDateString([], { month: 'short', day: 'numeric' })} ` : ''
  const duration = formatDuration((to ?? now).getTime() - from.getTime())
  return `${date}${clockTime(from)} → ${to ? clockTime(to) : 'now'} · ${duration}`
}

/** A local calendar day key (`2026-10-05`), for grouping by day. */
export function dayKey(iso: string): string {
  const d = new Date(iso)
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`
}

/** A `dayKey` as `Today`, `Yesterday`, `Sat, Oct 3`, or `Sat, Oct 3, 2025` outside this year. */
export function dayLabel(key: string, now = new Date()): string {
  const [y, m, d] = key.split('-').map(Number) as [number, number, number]
  const day = new Date(y, m - 1, d)
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate())
  const diff = Math.round((today.getTime() - day.getTime()) / 86_400_000)
  if (diff === 0) return 'Today'
  if (diff === 1) return 'Yesterday'
  return day.toLocaleDateString([], {
    weekday: 'short',
    month: 'short',
    day: 'numeric',
    ...(y !== now.getFullYear() && { year: 'numeric' }),
  })
}
