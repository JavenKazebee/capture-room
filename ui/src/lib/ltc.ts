import type { LtcState } from '@/types/generated/LtcState'

/**
 * The LTC channel choices for a feed with `channels` of audio (`null` when silent): off (0), then
 * each channel. A `current` pick beyond them stays listed so the form shows it; the node rejects it.
 */
export function ltcChannelOptions(channels: number | null, current: number | null) {
  const count = Math.max(channels ?? 0, current ?? 0)
  return [
    { value: 0, label: 'Off' },
    ...Array.from({ length: count }, (_, i) => {
      const n = i + 1
      const missing = n > (channels ?? 0)
      return { value: n, label: `Channel ${n}`, disabled: missing, reason: missing ? 'not in the feed' : undefined }
    }),
  ]
}

/** A warning for a source whose LTC isn't being read, or null. */
export function ltcWarning(state: LtcState | null | undefined): { short: string; title: string } | null {
  switch (state) {
    case 'missing':
      return { short: 'LTC missing', title: "No LTC on its channel since the feed started: recordings use the node's clock" }
    case 'lost':
      return { short: 'LTC lost', title: 'The LTC stopped: timecode counts on from the last LTC read' }
    default:
      return null
  }
}
