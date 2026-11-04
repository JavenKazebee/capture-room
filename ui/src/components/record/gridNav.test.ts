import { describe, expect, it } from 'vitest'
import { gridColumns, gridNavTarget } from '@/components/record/gridNav'

describe('gridColumns', () => {
  it('counts the columns auto-fill makes, gaps included', () => {
    // 3 × 300 + 2 × 16 = 932
    expect(gridColumns(932, 300, 16)).toBe(3)
    expect(gridColumns(931, 300, 16)).toBe(2)
    expect(gridColumns(1200, 200, 16)).toBe(5)
  })

  it('is one column when a tile is wider than the grid', () => {
    expect(gridColumns(500, 640, 16)).toBe(1)
  })

  it('is one column before the grid is measured', () => {
    expect(gridColumns(0, 300, 16)).toBe(1)
    expect(gridColumns(NaN, 300, 16)).toBe(1)
  })
})

describe('gridNavTarget', () => {
  // 3 columns, 8 tiles:
  // 0 1 2
  // 3 4 5
  // 6 7
  const nav = (key: string, from: number) => gridNavTarget(key, from, 8, 3)

  it('steps left and right in reading order, across rows', () => {
    expect(nav('ArrowRight', 0)).toBe(1)
    expect(nav('ArrowRight', 2)).toBe(3)
    expect(nav('ArrowLeft', 3)).toBe(2)
  })

  it('moves a row up and down', () => {
    expect(nav('ArrowDown', 1)).toBe(4)
    expect(nav('ArrowUp', 4)).toBe(1)
  })

  it('lands on the last tile when the row below is short', () => {
    expect(nav('ArrowDown', 5)).toBe(7)
  })

  it('stops at the edges', () => {
    expect(nav('ArrowLeft', 0)).toBe(0)
    expect(nav('ArrowRight', 7)).toBe(7)
    expect(nav('ArrowUp', 2)).toBe(2)
    expect(nav('ArrowDown', 6)).toBe(6)
  })

  it('follows the column count', () => {
    expect(gridNavTarget('ArrowDown', 1, 8, 4)).toBe(5)
    expect(gridNavTarget('ArrowDown', 1, 8, 1)).toBe(2)
  })

  it('jumps with Home and End', () => {
    expect(nav('Home', 5)).toBe(0)
    expect(nav('End', 1)).toBe(7)
  })

  it('starts from the near end when no tile had focus', () => {
    expect(nav('ArrowDown', -1)).toBe(0)
    expect(nav('ArrowRight', -1)).toBe(0)
    expect(nav('ArrowUp', -1)).toBe(7)
    expect(nav('ArrowLeft', -1)).toBe(7)
  })

  it('ignores other keys and empty grids', () => {
    expect(nav('a', 1)).toBeNull()
    expect(gridNavTarget('ArrowDown', -1, 0, 3)).toBeNull()
  })
})
