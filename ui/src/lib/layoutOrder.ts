/**
 * `order` with `key` moved to `target`'s place: before it when moving back, after it when
 * moving forward, so dropping on a tile always puts the dragged one where that tile was.
 * `null` when nothing would change or either key is missing.
 */
export function moveKey(order: readonly string[], key: string, target: string): string[] | null {
  const from = order.indexOf(key)
  const to = order.indexOf(target)
  if (from === -1 || to === -1 || from === to) return null
  const next = [...order]
  next.splice(from, 1)
  next.splice(to, 0, key)
  return next
}
