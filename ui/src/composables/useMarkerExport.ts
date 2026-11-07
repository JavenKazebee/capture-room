import { toast } from 'vue-sonner'
import { plural } from '@/lib/format'
import { markerEdl } from '@/lib/markers'
import { useRecordingsStore, type RecordingSession } from '@/stores/recordings'

/** Download the markers of `sessions` as one Resolve marker EDL, and say what it holds. */
export function useMarkerExport() {
  const recordings = useRecordingsStore()

  function exportMarkers(sessions: RecordingSession[]) {
    const date = new Date().toISOString().slice(0, 10)
    const out = markerEdl(
      sessions.map((s) => ({ key: `${s.node_id}/${s.id}`, label: recordings.sourceNameOf(s), markers: s.markers })),
      `Capture Room markers ${date}`,
    )
    if (!out.count) {
      toast.error('No markers to export', {
        description: out.skipped.length
          ? 'Markers need MOV recordings, which carry timecode.'
          : 'Press M while recording to mark a moment.',
      })
      return
    }
    const url = URL.createObjectURL(new Blob([out.edl], { type: 'text/plain' }))
    const a = document.createElement('a')
    a.href = url
    a.download = `${date} markers.edl`
    document.body.append(a)
    a.click()
    a.remove()
    setTimeout(() => URL.revokeObjectURL(url), 10_000)

    const left = out.skipped.map((s) => `${s.label} (${s.reason})`).join(', ')
    toast.success(`Exported ${plural(out.count, 'marker')}`, {
      description: [
        `In Resolve, right-click the timeline: Timelines → Import → Timeline Markers from EDL. First marker at ${out.first}.`,
        left && `Left out: ${left}.`,
      ]
        .filter(Boolean)
        .join(' '),
      duration: 10_000,
    })
  }

  return { exportMarkers }
}
