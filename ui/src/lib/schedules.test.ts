import { describe, expect, it } from 'vitest'
import {
  addToTime,
  monthlyFrom,
  ordinal,
  relativeFuture,
  scheduleInput,
  secondsBetween,
  weekdaysLabel,
  weekOfMonthLabel,
  whatLabel,
  whenLabel,
} from '@/lib/schedules'
import type { ScheduleDto } from '@/types/generated/ScheduleDto'

// Weekday bits, Monday first.
const MON = 1 << 0
const WED = 1 << 2
const SAT = 1 << 5
const SUN = 1 << 6

function schedule(overrides: Partial<ScheduleDto> = {}): ScheduleDto {
  return {
    id: 's1',
    name: 'Sunday service',
    enabled: true,
    kind: 'weekly',
    date: null,
    weekdays: SUN,
    month_day: null,
    month_week: null,
    start_time: '08:45',
    duration_s: 5400,
    actions: [],
    created_at: '2026-10-01T00:00:00Z',
    updated_at: '2026-10-01T00:00:00Z',
    next_start: null,
    next_end: null,
    last_run: null,
    ...overrides,
  }
}

describe('addToTime', () => {
  it('adds whole minutes', () => {
    expect(addToTime('08:45', 5400)).toBe('10:15')
    expect(addToTime('08:45', 89)).toBe('08:46')
  })
  it('wraps past midnight', () => {
    expect(addToTime('23:30', 3600)).toBe('00:30')
    expect(addToTime('00:00', 24 * 3600)).toBe('00:00')
  })
})

describe('secondsBetween', () => {
  it('measures forwards', () => {
    expect(secondsBetween('08:45', '10:15')).toBe(5400)
  })
  it('runs into the next day when the end is earlier or equal', () => {
    expect(secondsBetween('23:30', '00:30')).toBe(3600)
    expect(secondsBetween('10:00', '10:00')).toBe(24 * 3600)
  })
  it('inverts addToTime', () => {
    expect(addToTime('22:10', secondsBetween('22:10', '01:05'))).toBe('01:05')
  })
})

describe('weekdaysLabel', () => {
  it('names the common sets', () => {
    expect(weekdaysLabel(0x7f)).toBe('Every day')
    expect(weekdaysLabel(0x1f)).toBe('Weekdays')
    expect(weekdaysLabel(SAT | SUN)).toBe('Weekends')
  })
  it('lists other days Monday first', () => {
    expect(weekdaysLabel(SUN | WED | MON)).toBe('Mon, Wed, Sun')
    expect(weekdaysLabel(0)).toBe('')
  })
})

describe('ordinal', () => {
  it('picks the suffix', () => {
    expect([1, 2, 3, 4, 11, 12, 13, 15, 21, 22, 23, 31, 101, 111].map(ordinal)).toEqual([
      '1st', '2nd', '3rd', '4th', '11th', '12th', '13th', '15th', '21st', '22nd', '23rd', '31st', '101st', '111th',
    ])
  })
  it('says last for week -1', () => {
    expect(weekOfMonthLabel(2)).toBe('2nd')
    expect(weekOfMonthLabel(-1)).toBe('last')
  })
})

describe('monthlyFrom', () => {
  it('takes the day, the week and the weekday (Monday 0)', () => {
    // 2026-10-25 is the fourth Sunday of October.
    expect(monthlyFrom('2026-10-25')).toEqual({ day: 25, week: 4, weekday: 6 })
    expect(monthlyFrom('2026-10-05')).toEqual({ day: 5, week: 1, weekday: 0 })
  })
  it('calls a fifth weekday the last', () => {
    expect(monthlyFrom('2026-10-29')).toEqual({ day: 29, week: -1, weekday: 3 })
  })
})

describe('whenLabel', () => {
  it('labels a weekly schedule', () => {
    expect(whenLabel(schedule())).toBe('Sun 08:45–10:15')
    expect(whenLabel(schedule({ weekdays: 0x1f, duration_s: null }))).toBe('Weekdays 08:45')
  })
  it('labels a monthly schedule by day or by weekday', () => {
    expect(whenLabel(schedule({ kind: 'monthly', month_day: 15, start_time: '10:00', duration_s: null }))).toBe(
      'Monthly on the 15th · 10:00',
    )
    expect(whenLabel(schedule({ kind: 'monthly', month_week: -1, weekdays: SUN, start_time: '10:00', duration_s: 3600 }))).toBe(
      'Monthly on the last Sunday · 10:00–11:00',
    )
  })
  it('defaults a monthly weekday to the first', () => {
    expect(whenLabel(schedule({ kind: 'monthly', weekdays: MON, start_time: '09:00', duration_s: null }))).toBe(
      'Monthly on the 1st Monday · 09:00',
    )
  })
  it('labels a one-off by its date', () => {
    const date = new Date(2026, 9, 24).toLocaleDateString([], { weekday: 'short', month: 'short', day: 'numeric' })
    expect(whenLabel(schedule({ kind: 'once', date: '2026-10-24', start_time: '19:00', duration_s: null }))).toBe(`${date} 19:00`)
  })
})

describe('whatLabel', () => {
  it('joins the actions', () => {
    const label = whatLabel(
      [
        { kind: 'record', preset_id: 'p', targets: [{ node_id: 'n', source_id: 'a' }, { node_id: 'n', source_id: 'b' }] },
        { kind: 'playout', node_id: 'n', channel_id: 'c1', item_id: null, stop_at_end: true },
      ],
      (node, id) => `${node}/${id}`,
    )
    expect(label).toBe('Record 2 sources · Play n/c1')
  })
  it('is empty with no actions', () => {
    expect(whatLabel([], () => '')).toBe('')
  })
})

describe('relativeFuture', () => {
  const now = new Date('2026-10-05T12:00:00Z')
  const after = (secs: number) => new Date(now.getTime() + secs * 1000).toISOString()
  it('picks the unit', () => {
    expect(relativeFuture(after(40), now)).toBe('in 40 s')
    expect(relativeFuture(after(89), now)).toBe('in 89 s')
    expect(relativeFuture(after(90), now)).toBe('in 2 min')
    expect(relativeFuture(after(89 * 60), now)).toBe('in 89 min')
    expect(relativeFuture(after(3 * 3600), now)).toBe('in 3 h')
    expect(relativeFuture(after(2 * 86400), now)).toBe('in 2 days')
  })
  it('clamps the past to now', () => {
    expect(relativeFuture(after(-30), now)).toBe('in 0 s')
  })
})

describe('scheduleInput', () => {
  it('keeps only the saved fields', () => {
    const s = schedule({ next_start: '2026-10-05T08:45:00Z' })
    expect(scheduleInput(s)).toEqual({
      name: s.name,
      enabled: s.enabled,
      kind: s.kind,
      date: s.date,
      weekdays: s.weekdays,
      month_day: s.month_day,
      month_week: s.month_week,
      start_time: s.start_time,
      duration_s: s.duration_s,
      actions: s.actions,
    })
  })
})
