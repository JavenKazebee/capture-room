/**
 * Keyboard movement over a CSS grid of `repeat(auto-fill, minmax(min(100%, W), W))` tiles.
 * The column count isn't exposed by the browser, so it's worked out the way auto-fill does.
 */

/** How many `tile`-wide columns auto-fill fits in `width` with `gap` between them; at least one. */
export function gridColumns(width: number, tile: number, gap: number): number {
  if (!(width > 0) || !(tile > 0)) return 1
  return Math.max(1, Math.floor((width + gap) / (tile + gap)))
}

/**
 * The tile a navigation key moves focus to, as an index into the tiles shown, or null when the
 * key isn't one or there are none. `from` is -1 when no tile has had focus yet. Left/Right step in
 * reading order, Up/Down a row; moving down onto a short last row lands on its last tile. Stops
 * at the ends rather than wrapping.
 */
export function gridNavTarget(key: string, from: number, count: number, cols: number): number | null {
  if (!count) return null
  const last = count - 1
  if (from < 0 || from > last) {
    if (['ArrowRight', 'ArrowDown', 'Home'].includes(key)) return 0
    if (['ArrowLeft', 'ArrowUp', 'End'].includes(key)) return last
    return null
  }
  const c = Math.max(1, cols)
  switch (key) {
    case 'ArrowRight':
      return Math.min(from + 1, last)
    case 'ArrowLeft':
      return Math.max(from - 1, 0)
    case 'ArrowDown':
      // No row below: stay put.
      return Math.floor(from / c) === Math.floor(last / c) ? from : Math.min(from + c, last)
    case 'ArrowUp':
      return from - c >= 0 ? from - c : from
    case 'Home':
      return 0
    case 'End':
      return last
    default:
      return null
  }
}
