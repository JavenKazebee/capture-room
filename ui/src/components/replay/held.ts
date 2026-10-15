import { formatClock } from '@/lib/format'
import type { ReplayBuffer } from '@/stores/replay'

/** Held span of a buffer, as "4:32.0". */
export function held(b: ReplayBuffer) {
  return b.start_us !== null && b.end_us !== null ? formatClock((b.end_us - b.start_us) / 1000) : '0:00.0'
}
