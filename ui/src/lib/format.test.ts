import { describe, expect, it } from 'vitest'
import {
  dayKey,
  dayLabel,
  elapsedMs,
  formatBytes,
  formatClock,
  formatDuration,
  formatFingerprint,
  formatRate,
  formatTimeLeft,
  formatUptime,
  parseClock,
  plural,
} from '@/lib/format'

describe('plural', () => {
  it('adds an s except for one', () => {
    expect(plural(0, 'feed')).toBe('0 feeds')
    expect(plural(1, 'feed')).toBe('1 feed')
    expect(plural(2, 'feed')).toBe('2 feeds')
  })
})

describe('formatFingerprint', () => {
  it('groups by four, upper case', () => {
    expect(formatFingerprint('ab12cd34ef')).toBe('AB12 CD34 EF')
  })
  it('gives the first two groups when short', () => {
    expect(formatFingerprint('ab12cd34ef56', true)).toBe('AB12 CD34')
  })
  it('is empty for an empty fingerprint', () => {
    expect(formatFingerprint('')).toBe('')
  })
})

describe('formatDuration', () => {
  it('is MM:SS under an hour', () => {
    expect(formatDuration(0)).toBe('00:00')
    expect(formatDuration(59_999)).toBe('00:59')
    expect(formatDuration(61_000)).toBe('01:01')
  })
  it('is HH:MM:SS from an hour on', () => {
    expect(formatDuration(3_600_000)).toBe('01:00:00')
    expect(formatDuration(37 * 3_600_000 + 5_000)).toBe('37:00:05')
  })
  it('clamps negatives to zero', () => {
    expect(formatDuration(-5_000)).toBe('00:00')
  })
})

describe('formatUptime', () => {
  it('shows minutes, and hours from an hour on', () => {
    expect(formatUptime(59)).toBe('0m')
    expect(formatUptime(12 * 60)).toBe('12m')
    expect(formatUptime(3 * 3600 + 12 * 60 + 30)).toBe('3h 12m')
  })
})

describe('formatBytes', () => {
  it('uses decimal units', () => {
    expect(formatBytes(0)).toBe('0 B')
    expect(formatBytes(512)).toBe('512 B')
    expect(formatBytes(999)).toBe('999 B')
    expect(formatBytes(1000)).toBe('1.0 KB')
    expect(formatBytes(1.5e9)).toBe('1.5 GB')
  })
  it('drops the decimal from 100 up', () => {
    expect(formatBytes(120e9)).toBe('120 GB')
  })
  it('stops at TB', () => {
    expect(formatBytes(2.5e15)).toBe('2500 TB')
  })
  it('formats a rate', () => {
    expect(formatRate(1.4e6)).toBe('1.4 MB/s')
  })
})

describe('formatTimeLeft', () => {
  it('is coarse', () => {
    expect(formatTimeLeft(2 * 86400 + 4 * 3600 + 59 * 60)).toBe('2d 4h')
    expect(formatTimeLeft(3 * 3600 + 20 * 60)).toBe('3h 20m')
    expect(formatTimeLeft(45 * 60 + 59)).toBe('45m')
    expect(formatTimeLeft(59)).toBe('<1m')
    expect(formatTimeLeft(0)).toBe('<1m')
  })
})

describe('elapsedMs', () => {
  const now = new Date('2026-10-05T12:00:10Z')
  it('measures to the end', () => {
    expect(elapsedMs('2026-10-05T12:00:00Z', '2026-10-05T12:00:05Z', now)).toBe(5000)
  })
  it('measures to now while running', () => {
    expect(elapsedMs('2026-10-05T12:00:00Z', null, now)).toBe(10_000)
  })
})

describe('dayKey', () => {
  it('is the local calendar day', () => {
    expect(dayKey(new Date(2026, 9, 5, 23, 59).toISOString())).toBe('2026-10-05')
    expect(dayKey(new Date(2026, 0, 1, 0, 0).toISOString())).toBe('2026-01-01')
  })
})

describe('dayLabel', () => {
  const now = new Date(2026, 9, 5, 0, 30)
  it('names today and yesterday', () => {
    expect(dayLabel('2026-10-05', now)).toBe('Today')
    expect(dayLabel('2026-10-04', now)).toBe('Yesterday')
  })
  it('counts days across a month boundary', () => {
    expect(dayLabel('2026-11-01', new Date(2026, 10, 1, 9))).toBe('Today')
    expect(dayLabel('2026-10-31', new Date(2026, 10, 1, 9))).toBe('Yesterday')
  })
  it('dates older days, with the year only outside this one', () => {
    const fmt = (d: Date, year: boolean) =>
      d.toLocaleDateString([], { weekday: 'short', month: 'short', day: 'numeric', ...(year && { year: 'numeric' }) })
    expect(dayLabel('2026-10-03', now)).toBe(fmt(new Date(2026, 9, 3), false))
    expect(dayLabel('2025-10-03', now)).toBe(fmt(new Date(2025, 9, 3), true))
    expect(dayLabel('2025-10-03', now)).toContain('2025')
  })
})

describe('formatClock', () => {
  it('is M:SS.d under an hour', () => {
    expect(formatClock(0)).toBe('0:00.0')
    expect(formatClock(12_345)).toBe('0:12.3')
    expect(formatClock(61_990)).toBe('1:01.9')
  })
  it('is H:MM:SS.d from an hour on', () => {
    expect(formatClock(3_600_000)).toBe('1:00:00.0')
    expect(formatClock(3_725_400)).toBe('1:02:05.4')
  })
  it('clamps negatives to zero', () => {
    expect(formatClock(-100)).toBe('0:00.0')
  })
})

describe('parseClock', () => {
  it('reads seconds', () => {
    expect(parseClock('12.5')).toBe(12_500)
    expect(parseClock(' 7 ')).toBe(7000)
    expect(parseClock('3.')).toBe(3000)
  })
  it('reads M:SS and H:MM:SS', () => {
    expect(parseClock('1:02.5')).toBe(62_500)
    expect(parseClock('1:02:05.4')).toBe(3_725_400)
  })
  it('rounds to the millisecond', () => {
    expect(parseClock('0.0004')).toBe(0)
    expect(parseClock('0.0006')).toBe(1)
  })
  it('rejects anything else', () => {
    for (const bad of ['', ' ', 'abc', '1:', ':5', '1:2:3:4', '-1', '1e3', '1:2.5.6']) expect(parseClock(bad), bad).toBeNull()
  })
  it('round-trips formatClock', () => {
    for (const ms of [0, 12_300, 61_900, 3_725_400]) expect(parseClock(formatClock(ms))).toBe(ms)
  })
})
