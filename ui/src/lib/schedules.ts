import { plural } from '@/lib/format'
import { WEEKDAYS } from '@/stores/schedules'
import type { ScheduleAction } from '@/types/generated/ScheduleAction'
import type { ScheduleDto } from '@/types/generated/ScheduleDto'

const pad = (n: number) => String(n).padStart(2, '0')

/** `HH:MM` plus `secs`, wrapping past midnight. */
export function addToTime(time: string, secs: number): string {
  const [h, m] = time.split(':').map(Number) as [number, number]
  const mins = (h * 60 + m + Math.round(secs / 60)) % (24 * 60)
  return `${pad(Math.floor(mins / 60))}:${pad(mins % 60)}`
}

/** Seconds from `start` to `end` (both `HH:MM`), the next day if `end` is earlier. */
export function secondsBetween(start: string, end: string): number {
  const toMin = (t: string) => {
    const [h, m] = t.split(':').map(Number) as [number, number]
    return h * 60 + m
  }
  const diff = toMin(end) - toMin(start)
  return (diff > 0 ? diff : diff + 24 * 60) * 60
}

/** `Sun, Wed`, `Every day`, `Weekdays`. */
export function weekdaysLabel(bits: number): string {
  if ((bits & 0x7f) === 0x7f) return 'Every day'
  if ((bits & 0x7f) === 0x1f) return 'Weekdays'
  if ((bits & 0x7f) === 0x60) return 'Weekends'
  return WEEKDAYS.filter((_, i) => bits & (1 << i)).join(', ')
}

/** `Sun 08:45–10:15`, or `Oct 24 19:00` for a one-off without an end. */
export function whenLabel(s: Pick<ScheduleDto, 'kind' | 'date' | 'weekdays' | 'start_time' | 'duration_s'>): string {
  const days =
    s.kind === 'weekly'
      ? weekdaysLabel(s.weekdays)
      : s.date
        ? new Date(`${s.date}T00:00`).toLocaleDateString([], { weekday: 'short', month: 'short', day: 'numeric' })
        : ''
  const end = s.duration_s ? `–${addToTime(s.start_time, s.duration_s)}` : ''
  return `${days} ${s.start_time}${end}`
}

/** `3 sources · Pre-roll on Channel 1`. */
export function whatLabel(actions: ScheduleAction[], channelName: (nodeId: string, id: string) => string): string {
  return actions
    .map((a) =>
      a.kind === 'record' ? `Record ${plural(a.targets.length, 'source')}` : `Play ${channelName(a.node_id, a.channel_id)}`,
    )
    .join(' · ')
}

/** `in 3 h`, `in 2 days`, `in 40 s`. */
export function relativeFuture(iso: string, now: Date): string {
  const secs = Math.max(0, Math.round((new Date(iso).getTime() - now.getTime()) / 1000))
  if (secs < 90) return `in ${secs} s`
  const mins = Math.round(secs / 60)
  if (mins < 90) return `in ${mins} min`
  const hours = Math.round(mins / 60)
  if (hours < 36) return `in ${hours} h`
  return `in ${Math.round(hours / 24)} days`
}
