import { describe, expect, it } from 'vitest'
import { rowNavTarget } from '@/components/common/rowNav'

describe('rowNavTarget', () => {
  it('moves one row with the arrows', () => {
    expect(rowNavTarget('ArrowDown', 1, 5)).toBe(2)
    expect(rowNavTarget('ArrowUp', 1, 5)).toBe(0)
  })

  it('stops at the ends', () => {
    expect(rowNavTarget('ArrowDown', 4, 5)).toBe(4)
    expect(rowNavTarget('ArrowUp', 0, 5)).toBe(0)
  })

  it('starts from the near end when no row had focus', () => {
    expect(rowNavTarget('ArrowDown', -1, 5)).toBe(0)
    expect(rowNavTarget('ArrowUp', -1, 5)).toBe(4)
  })

  it('jumps with Home and End', () => {
    expect(rowNavTarget('Home', 3, 5)).toBe(0)
    expect(rowNavTarget('End', 0, 5)).toBe(4)
  })

  it('ignores other keys and empty tables', () => {
    expect(rowNavTarget('Enter', 1, 5)).toBeNull()
    expect(rowNavTarget('a', 1, 5)).toBeNull()
    expect(rowNavTarget('ArrowDown', -1, 0)).toBeNull()
  })
})
