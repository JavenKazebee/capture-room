import { pad, plural } from '@/lib/format'
import { WEEKDAYS } from '@/stores/schedules'
import type { ScheduleAction } from '@/types/generated/ScheduleAction'
import type { ScheduleDto } from '@/types/generated/ScheduleDto'
import type { ScheduleInput } from '@/types/generated/ScheduleInput'

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

/** Monday first, as `WEEKDAYS`. */
export const WEEKDAY_NAMES = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'] as const

/** `1st`, `2nd`, `15th`, `31st`. */
export function ordinal(n: number): string {
  const tens = n % 100
  const suffix = tens >= 11 && tens <= 13 ? 'th' : (['th', 'st', 'nd', 'rd'][n % 10] ?? 'th')
  return `${n}${suffix}`
}

/** A monthly `month_week`: `2nd`, or `last` for -1. */
export function weekOfMonthLabel(week: number): string {
  return week < 0 ? 'last' : ordinal(week)
}

/**
 * A monthly schedule seeded from a `YYYY-MM-DD` date: its day of the month,
 * and which of its weekday it is (`last` for a fifth). Weekday is Monday 0.
 */
export function monthlyFrom(date: string): { day: number; week: number; weekday: number } {
  const d = new Date(`${date}T00:00`)
  const day = d.getDate()
  const week = Math.ceil(day / 7)
  return { day, week: week > 4 ? -1 : week, weekday: (d.getDay() + 6) % 7 }
}

type When = Pick<ScheduleDto, 'kind' | 'date' | 'weekdays' | 'month_day' | 'month_week' | 'start_time' | 'duration_s'>

/** `on the 15th`, `on the last Sunday`. */
function monthlyLabel(s: When): string {
  if (s.month_day != null) return `on the ${ordinal(s.month_day)}`
  const weekday = WEEKDAY_NAMES.find((_, i) => s.weekdays & (1 << i)) ?? ''
  return `on the ${weekOfMonthLabel(s.month_week ?? 1)} ${weekday}`
}

/**
 * `Sun 08:45–10:15`, `Monthly on the last Sunday · 10:00`, or `Oct 24 19:00`
 * for a one-off without an end.
 */
export function whenLabel(s: When): string {
  const end = s.duration_s ? `–${addToTime(s.start_time, s.duration_s)}` : ''
  if (s.kind === 'monthly') return `Monthly ${monthlyLabel(s)} · ${s.start_time}${end}`
  const days =
    s.kind === 'weekly'
      ? weekdaysLabel(s.weekdays)
      : s.date
        ? new Date(`${s.date}T00:00`).toLocaleDateString([], { weekday: 'short', month: 'short', day: 'numeric' })
        : ''
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

/** The fields of a schedule that saving it takes, to change one and save the rest unchanged. */
export function scheduleInput(s: ScheduleDto): ScheduleInput {
  const { name, enabled, kind, date, weekdays, month_day, month_week, start_time, duration_s, actions } = s
  return { name, enabled, kind, date, weekdays, month_day, month_week, start_time, duration_s, actions }
}
