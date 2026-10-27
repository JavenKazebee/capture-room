import { describe, expect, it } from 'vitest'
import { nodeNowUs, replaySpan, type AnchoredBuffer } from '@/lib/replaySpan'

const S = 1_000_000

function buffer(overrides: Partial<AnchoredBuffer> = {}): AnchoredBuffer {
  return {
    minutes: 1,
    encoder: 'jpegenc',
    error: null,
    start_us: 100 * S,
    end_us: 120 * S,
    now_us: 121 * S,
    fetched_at: 5000,
    ...overrides,
  }
}

describe('nodeNowUs', () => {
  it('runs the node clock on from when the buffer was reported', () => {
    expect(nodeNowUs(buffer(), 5000)).toBe(121 * S)
    expect(nodeNowUs(buffer(), 7500)).toBe(123.5 * S)
  })
})

describe('replaySpan', () => {
  it('is null while nothing is recorded', () => {
    expect(replaySpan(buffer({ start_us: null, end_us: null }), 9000)).toBeNull()
  })

  it('runs the live edge on while recording, keeping the start until full', () => {
    expect(replaySpan(buffer(), 5000)).toEqual({ start: 100 * S, end: 120 * S })
    expect(replaySpan(buffer(), 15_000)).toEqual({ start: 100 * S, end: 130 * S })
  })

  it('trims the start once the buffer is full', () => {
    // 40 s on, it holds 60 s: a minute.
    expect(replaySpan(buffer(), 45_000)).toEqual({ start: 100 * S, end: 160 * S })
    expect(replaySpan(buffer(), 55_000)).toEqual({ start: 110 * S, end: 170 * S })
  })

  it('stands still when not recording', () => {
    expect(replaySpan(buffer({ encoder: null }), 55_000)).toEqual({ start: 100 * S, end: 120 * S })
    expect(replaySpan(buffer({ error: 'the source isn\'t available' }), 55_000)).toEqual({ start: 100 * S, end: 120 * S })
  })
})
