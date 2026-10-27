/**
 * The row a navigation key moves focus to, as an index into the rows shown, or null when the
 * key isn't one or there are no rows. `from` is -1 when no row has had focus yet. Stops at the
 * ends rather than wrapping, like a listbox.
 */
export function rowNavTarget(key: string, from: number, count: number): number | null {
  if (!count) return null
  switch (key) {
    case 'ArrowDown':
      return from === -1 ? 0 : Math.min(from + 1, count - 1)
    case 'ArrowUp':
      return from === -1 ? count - 1 : Math.max(from - 1, 0)
    case 'Home':
      return 0
    case 'End':
      return count - 1
    default:
      return null
  }
}
