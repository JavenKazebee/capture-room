import type { ReplayBufferDto } from '@/types/generated/ReplayBufferDto'

/**
 * A replay buffer as its node last reported it, and `performance.now()` then: the node's clock
 * offset from ours. The node reports a buffer only when it changes in a way that can't be foreseen,
 * not as each segment lands, so between reports its span is run on from here.
 */
export type AnchoredBuffer = Pick<ReplayBufferDto, 'minutes' | 'encoder' | 'error' | 'start_us' | 'end_us' | 'now_us'> & {
  fetched_at: number
}

/** The node clock (µs) at `at`, a `performance.now()` time. */
export function nodeNowUs(b: AnchoredBuffer, at: number) {
  return b.now_us + Math.round((at - b.fetched_at) * 1000)
}

/**
 * What can be replayed at `at`, oldest to newest frame on the node clock; `null` while empty.
 * While it records, the newest frame keeps its distance behind the node clock, and once the buffer
 * is full the oldest follows `minutes` behind that. Up to a segment off, which the node clamps.
 */
export function replaySpan(b: AnchoredBuffer, at: number): { start: number; end: number } | null {
  if (b.start_us === null || b.end_us === null) return null
  if (b.encoder === null || b.error !== null) return { start: b.start_us, end: b.end_us }
  const end = b.end_us + (nodeNowUs(b, at) - b.now_us)
  return { start: Math.max(b.start_us, end - b.minutes * 60_000_000), end }
}
