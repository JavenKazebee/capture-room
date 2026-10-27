import { describe, expect, it } from 'vitest'
import { ref } from 'vue'
import { useSelection } from '@/composables/useSelection'

const KEYS = ['a', 'b', 'c', 'd', 'e']
const keys = (s: ReturnType<typeof useSelection>) => [...s.selected.value].sort()

describe('useSelection', () => {
  it('selects just one on a plain click', () => {
    const s = useSelection(KEYS)
    s.select('a')
    s.select('c')
    expect(keys(s)).toEqual(['c'])
    expect(s.focusedKey.value).toBe('c')
    expect(s.count.value).toBe(1)
  })

  it('toggles one in or out', () => {
    const s = useSelection(KEYS)
    s.select('a', 'toggle')
    s.select('c', 'toggle')
    expect(keys(s)).toEqual(['a', 'c'])
    expect(s.focusedKey.value).toBe('c')
    s.select('c', 'toggle')
    expect(keys(s)).toEqual(['a'])
    // Toggling the focused key out moves focus to the last one still selected.
    expect(s.focusedKey.value).toBe('a')
    s.select('a', 'toggle')
    expect(s.count.value).toBe(0)
    expect(s.focusedKey.value).toBeNull()
  })

  it('keeps focus when toggling out another key', () => {
    const s = useSelection(KEYS)
    s.select('a', 'toggle')
    s.select('b', 'toggle')
    s.select('a', 'toggle')
    expect(s.focusedKey.value).toBe('b')
  })

  it('selects a range from the focused key, either way', () => {
    const s = useSelection(KEYS)
    s.select('b')
    s.selectRange('d')
    expect(keys(s)).toEqual(['b', 'c', 'd'])
    expect(s.focusedKey.value).toBe('d')

    const t = useSelection(KEYS)
    t.select('d')
    t.selectRange('b')
    expect(keys(t)).toEqual(['b', 'c', 'd'])
  })

  it('adds a range to what is already selected', () => {
    const s = useSelection(KEYS)
    s.select('a')
    s.select('d', 'toggle')
    s.selectRange('e')
    expect(keys(s)).toEqual(['a', 'd', 'e'])
  })

  it('selects just the key when nothing is focused or a key is not shown', () => {
    const s = useSelection(KEYS)
    s.selectRange('c')
    expect(keys(s)).toEqual(['c'])
    s.selectRange('z')
    expect(keys(s)).toEqual(['z'])
    expect(s.focusedKey.value).toBe('z')
    s.selectRange('b')
    expect(keys(s)).toEqual(['b'])
  })

  it('takes the shown keys at the call', () => {
    const s = useSelection()
    s.select('e')
    s.selectRange('a', ['e', 'd', 'c', 'b', 'a'])
    expect(keys(s)).toEqual(KEYS)
  })

  it('follows a reactive order', () => {
    const order = ref(['a', 'b', 'c'])
    const s = useSelection(order)
    order.value = ['c', 'b', 'a']
    s.select('c')
    s.selectRange('b')
    expect(keys(s)).toEqual(['b', 'c'])
    s.selectAll()
    expect(keys(s)).toEqual(['a', 'b', 'c'])
  })

  it('selects all, focusing the first only if nothing was', () => {
    const s = useSelection(KEYS)
    s.selectAll()
    expect(s.count.value).toBe(5)
    expect(s.focusedKey.value).toBe('a')

    const t = useSelection(KEYS)
    t.select('c')
    t.selectAll()
    expect(t.focusedKey.value).toBe('c')

    const empty = useSelection()
    empty.selectAll()
    expect(empty.focusedKey.value).toBeNull()
  })

  it('clears', () => {
    const s = useSelection(KEYS)
    s.selectAll()
    s.clearSelection()
    expect(s.count.value).toBe(0)
    expect(s.focusedKey.value).toBeNull()
  })

  it('deselects keys, moving focus off a removed one', () => {
    const s = useSelection(KEYS)
    s.select('a', 'toggle')
    s.select('b', 'toggle')
    s.select('c', 'toggle')
    s.deselect(['c', 'z'])
    expect(keys(s)).toEqual(['a', 'b'])
    expect(s.focusedKey.value).toBe('b')
    s.deselect(['a'])
    expect(s.focusedKey.value).toBe('b')
  })

  it('leaves the set alone when nothing changes', () => {
    const s = useSelection(KEYS)
    s.select('a')
    const before = s.selected.value
    s.deselect(['z'])
    s.retain(KEYS)
    expect(s.selected.value).toBe(before)
  })

  it('retains only keys that still exist', () => {
    const s = useSelection(KEYS)
    s.selectAll()
    s.select('e', 'toggle')
    s.select('e', 'toggle')
    s.retain(new Set(['a', 'c']))
    expect(keys(s)).toEqual(['a', 'c'])
    expect(s.focusedKey.value).toBe('c')
  })
})
