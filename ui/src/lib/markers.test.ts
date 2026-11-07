import { describe, expect, it } from 'vitest'
import { framesTimecode, markerEdl, pressTime, timecodeFrames, type MarkedRecording } from '@/lib/markers'
import type { MarkerDto } from '@/types/generated/MarkerDto'

const DF = '30000/1001'

function marker(press: string, timecode: string | null, overrides: Partial<MarkerDto> = {}): MarkerDto {
  return {
    id: `${press}-${Math.random()}`,
    press_id: press,
    at_us: 0,
    timecode,
    fps: timecode ? DF : null,
    note: '',
    color: 'blue',
    created_at: '',
    ...overrides,
  }
}

function rec(label: string, markers: MarkerDto[]): MarkedRecording {
  return { key: label, label, markers }
}

describe('pressTime', () => {
  it('steps back from the middle of the request to the keypress', () => {
    const now = { domain: 'controller:a', time_us: 10_000_000 }
    // Pressed at 100 ms, asked at 105, answered at 125: read at ~115, 15 ms after the press.
    expect(pressTime(now, 100, 105, 125)).toEqual({ domain: 'controller:a', time_us: 9_985_000 })
  })
})

describe('timecode frames', () => {
  it('counts non-drop frames', () => {
    expect(timecodeFrames('00:00:01:00', '25/1')).toBe(25)
    expect(framesTimecode(25 * 3600 + 3, '25/1')).toBe('01:00:00:03')
  })

  it('skips two frame numbers each minute but every tenth at 29.97', () => {
    expect(timecodeFrames('00:01:00;02', DF)).toBe(1800)
    expect(framesTimecode(1799, DF)).toBe('00:00:59;29')
    expect(framesTimecode(1800, DF)).toBe('00:01:00;02')
    expect(framesTimecode(17982, DF)).toBe('00:10:00;00')
    for (const tc of ['14:32:07;12', '23:59:59;29', '10:09:00;02', '00:00:00;00']) {
      expect(framesTimecode(timecodeFrames(tc, DF)!, DF)).toBe(tc)
    }
  })

  it('wraps at midnight', () => {
    expect(framesTimecode(timecodeFrames('23:59:59;29', DF)! + 1, DF)).toBe('00:00:00;00')
  })
})

describe('markerEdl', () => {
  it('lists each press once, in time order, as Resolve reads it', () => {
    const out = markerEdl(
      [
        rec('Cam A', [marker('p2', '14:40:00;00', { note: 'Cut to wide', color: 'red' }), marker('p1', '14:32:07;12')]),
        rec('Cam B', [marker('p2', '14:40:00;00'), marker('p1', '14:32:07;12')]),
      ],
      'Sunday 2026-11-06',
    )
    expect(out.count).toBe(2)
    expect(out.first).toBe('14:32:07;12')
    expect(out.skipped).toEqual([])
    expect(out.edl).toBe(
      [
        'TITLE: Sunday 2026-11-06',
        'FCM: DROP FRAME',
        '',
        '001  001      V     C        14:32:07;12 14:32:07;13 14:32:07;12 14:32:07;13  ',
        ' |C:ResolveColorBlue |M:Marker |D:1',
        '',
        '002  001      V     C        14:40:00;00 14:40:00;01 14:40:00;00 14:40:00;01  ',
        ' |C:ResolveColorRed |M:Cut to wide |D:1',
        '',
      ].join('\n'),
    )
  })

  it('names the recording a one-camera press marked, and cleans notes', () => {
    const out = markerEdl(
      [rec('Cam A', [marker('p1', '10:00:00;00', { note: 'Look | here\nnow' })]), rec('Cam B', [])],
      't',
    )
    expect(out.edl).toContain('|M:Cam A: Look here now |D:1')
  })

  it('leaves out recordings with no timecode, another rate or another timecode', () => {
    const out = markerEdl(
      [
        rec('A', [marker('p1', '10:00:00;00')]),
        rec('B', [marker('p1', '10:00:00;00')]),
        rec('MP4', [marker('p1', null)]),
        rec('PAL', [marker('p1', '10:00:00:00', { fps: '25/1' })]),
        rec('Own TC', [marker('p1', '03:12:00;10')]),
      ],
      't',
    )
    expect(out.count).toBe(1)
    expect(out.edl).toContain('10:00:00;00 10:00:00;01')
    expect(out.skipped).toEqual([
      { label: 'MP4', reason: 'no timecode' },
      { label: 'PAL', reason: 'another frame rate' },
      { label: 'Own TC', reason: 'another timecode' },
    ])
  })

  it('is empty without markers', () => {
    const out = markerEdl([rec('A', [])], 't')
    expect(out).toMatchObject({ count: 0, first: null, fps: null, skipped: [] })
    expect(out.edl).toBe('TITLE: t\nFCM: NON-DROP FRAME\n')
  })
})
