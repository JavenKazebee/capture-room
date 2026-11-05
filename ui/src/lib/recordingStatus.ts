import { CODECS } from '@/lib/codecs'
import { plural } from '@/lib/format'
import type { RecordingSessionDto } from '@/types/generated/RecordingSessionDto'

/** How each recording status is shown, and its sort order (live first). */
export const RECORDING_STATUS = {
  active: { label: 'Recording', dot: 'tally', order: 0 },
  error: { label: 'Failed', dot: 'error', order: 1 },
  stopped: { label: 'Finished', dot: 'off', order: 2 },
} as const

/** The codecs a session recorded, e.g. "ProRes 422 HQ + H.264". */
export function recordingFormats(s: Pick<RecordingSessionDto, 'outputs' | 'output_paths'>) {
  if (!s.outputs.length) return plural(s.output_paths.length, 'output')
  return s.outputs.map((o) => CODECS[o.codec]).join(' + ')
}
