import { describe, expect, it } from 'vitest'
import { moveKey } from './layoutOrder'

describe('moveKey', () => {
  it('takes the target’s place moving forward', () => {
    expect(moveKey(['a', 'b', 'c', 'd'], 'a', 'c')).toEqual(['b', 'c', 'a', 'd'])
  })

  it('takes the target’s place moving back', () => {
    expect(moveKey(['a', 'b', 'c', 'd'], 'd', 'b')).toEqual(['a', 'd', 'b', 'c'])
  })

  it('leaves keys between that the caller can’t see in place', () => {
    // `x` is on a node that's offline: it stays before `c`.
    expect(moveKey(['a', 'x', 'c'], 'a', 'c')).toEqual(['x', 'c', 'a'])
  })

  it('returns null for no move or a missing key', () => {
    expect(moveKey(['a', 'b'], 'a', 'a')).toBeNull()
    expect(moveKey(['a', 'b'], 'a', 'z')).toBeNull()
  })
})
