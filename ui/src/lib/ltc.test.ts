import { describe, expect, it } from 'vitest'
import { ltcChannelOptions, ltcWarning } from './ltc'

describe('ltcChannelOptions', () => {
  it('lists off and each channel', () => {
    expect(ltcChannelOptions(2, null).map((o) => o.value)).toEqual([0, 1, 2])
  })

  it('offers only off for a silent feed', () => {
    expect(ltcChannelOptions(null, null).map((o) => o.value)).toEqual([0])
  })

  it('keeps a pick the feed no longer has, disabled', () => {
    const options = ltcChannelOptions(2, 8)
    expect(options).toHaveLength(9)
    expect(options[8]).toMatchObject({ value: 8, disabled: true })
    expect(options[2]).toMatchObject({ value: 2, disabled: false })
  })
})

describe('ltcWarning', () => {
  it('warns only when LTC is missing or lost', () => {
    expect(ltcWarning('missing')?.short).toBe('LTC missing')
    expect(ltcWarning('lost')?.short).toBe('LTC lost')
    expect(ltcWarning('locked')).toBeNull()
    expect(ltcWarning('source_timecode')).toBeNull()
    expect(ltcWarning(null)).toBeNull()
  })
})
