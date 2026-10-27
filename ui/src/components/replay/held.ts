import { formatClock } from '@/lib/format'
import { replaySpan } from '@/lib/replaySpan'
import { useSecondClock } from '@/composables/useSecondClock'
import type { ReplayBuffer } from '@/stores/replay'

/** Held span of a buffer, as "4:32.0". */
export function held(b: ReplayBuffer, at = performance.now()) {
  const span = replaySpan(b, at)
  return span ? formatClock((span.end - span.start) / 1000) : '0:00.0'
}

/** `held`, read again each second: the span runs on between the node's reports. */
export function useHeld() {
  const second = useSecondClock()
  return (b: ReplayBuffer) => {
    void second.value
    return held(b)
  }
}
