import { computed, type Ref } from 'vue'
import { audioLevels, type Source } from '@/stores/sources'
import { useRecordingsStore } from '@/stores/recordings'
import { useRecordDeskStore } from '@/stores/recordDesk'
import { useSecondClock } from '@/composables/useSecondClock'
import { elapsedMs, formatDuration } from '@/lib/format'
import { formatLabel } from '@/lib/sourceFormat'
import { LINK } from '@/lib/linkState'

/** What a feed's tile and its expanded view both show: record state, levels, signal and format. */
export function useFeedState(source: Readonly<Ref<Source>>) {
  const desk = useRecordDeskStore()
  const recordings = useRecordingsStore()

  const session = computed(() => recordings.activeForSource(source.value.node_id, source.value.id))
  const busy = computed(() => desk.busy.has(source.value.key))
  const channels = computed(() => audioLevels.get(source.value.key) ?? [])

  const preset = computed({
    get: () => desk.presetIdOf(source.value.key),
    set: (id: string) => desk.setPreset(source.value.key, id),
  })

  const now = useSecondClock()
  const duration = computed(() =>
    session.value ? formatDuration(elapsedMs(session.value.started_at, null, now.value)) : '',
  )

  /** A live source that isn't delivering frames (it plays black and silence). */
  const noSignal = computed(() => {
    const link = source.value.link
    return link && link !== 'live' && !source.value.error ? LINK[link] : null
  })

  const format = computed(() => {
    const c = source.value.capabilities
    return c
      ? formatLabel({ width: c.max_width, height: c.max_height, fps_num: c.fps_num, fps_den: c.fps_den })
      : null
  })

  return { session, busy, channels, preset, duration, noSignal, format }
}
