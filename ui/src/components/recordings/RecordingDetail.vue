<script setup lang="ts">
import { computed, ref } from 'vue'
import { useStorage } from '@vueuse/core'
import { useSecondClock } from '@/composables/useSecondClock'
import { useRecordingPlayer } from '@/composables/useRecordingPlayer'
import { toast } from 'vue-sonner'
import { ChevronRight, Play, Trash2, X } from '@lucide/vue'
import { clockTimeFine, dateTime, elapsedMs, formatDuration, formatTimeRange, plural } from '@/lib/format'
import { CODECS, CONTAINERS } from '@/lib/codecs'
import { notifyError } from '@/lib/notify'
import { useNodesStore } from '@/stores/nodes'
import { outputFiles, previewOutput, useRecordingsStore, type RecordingSession } from '@/stores/recordings'
import type { SessionAudio } from '@/types/generated/SessionAudio'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import KeyValueList from '@/components/common/KeyValueList.vue'
import StatusDot from '@/components/common/StatusDot.vue'
import RecordingOutputs from './RecordingOutputs.vue'

const props = defineProps<{ session: RecordingSession }>()
defineEmits<{ close: [] }>()

const recordings = useRecordingsStore()
const nodes = useNodesStore()
const now = useSecondClock()

const s = computed(() => props.session)
const live = computed(() => s.value.status === 'active')

const AUDIO: Record<SessionAudio, string> = { pcm: 'PCM', aac: 'AAC', opus: 'Opus' }

const outputs = computed(() =>
  s.value.output_paths.map((path, i) => {
    const o = s.value.outputs[i]
    return {
      name: o?.name ?? `Output ${i + 1}`,
      format: o ? `${CODECS[o.codec]} · ${AUDIO[o.audio]} · ${CONTAINERS[o.container]}` : null,
      files: outputFiles(s.value, i),
      dropped: s.value.dropped_frames[i] ?? 0,
      path,
    }
  }),
)

const player = useRecordingPlayer(s, outputs)
const { playError, autoplay, playingUrl, playingCaption, playingDeleted, canPlay, isPlaying, play, togglePlay } = player
defineExpose({ togglePlay })

/** Why there's no preview, when there isn't one. */
const noPreview = computed(() => {
  if (live.value) return 'Still recording. Files can be previewed once the recording stops.'
  if (!s.value.outputs.length) return 'Recorded before previews were available, so its formats are unknown. Download a file to play it.'
  if (previewOutput(s.value) === -1)
    return "None of this session's outputs play in a browser. Add an H.264 output with AAC audio in .mp4 or .mov to its preset to preview recordings."
  return null
})

function when(iso: string | null) {
  return iso ? dateTime(iso, { year: true }) : null
}

const duration = computed(() => formatDuration(elapsedMs(s.value.started_at, s.value.stopped_at, now.value)))

const STATUS = {
  active: { label: 'Recording', dot: 'tally' },
  stopped: { label: 'Finished', dot: 'off' },
  error: { label: 'Failed', dot: 'error' },
} as const

const whenText = computed(() => formatTimeRange(s.value.started_at, s.value.stopped_at, now.value, { withDate: true }))

/** Exact times and IDs are tucked away; the summary row covers the rest. */
const detailsOpen = useStorage('cr.recordings.detailsOpen', false)

const details = computed(() => [
  { label: 'Status', value: STATUS[s.value.status].label },
  { label: 'Node', value: nodes.labelOf(s.value.node_id) },
  { label: 'Preset', value: recordings.presetNameOf(s.value) },
  { label: 'Started', value: when(s.value.started_at), mono: true },
  { label: 'Stopped', value: when(s.value.stopped_at), mono: true },
  { label: 'Duration', value: duration.value, mono: true },
  ...(s.value.clock
    ? [
        { label: 'First frame', value: firstFrame(s.value.clock.first_frame_utc), mono: true },
        { label: 'Clock', value: clockLabel(s.value.clock.domain, !!s.value.clock.start_at_us) },
      ]
    : []),
  { label: 'Session', value: s.value.id, mono: true, copy: true },
])

/** When the first frame was captured, to the millisecond: what lines recordings up. */
function firstFrame(iso: string | null) {
  return iso ? clockTimeFine(iso, 3) : null
}

function clockLabel(domain: string, synchronized: boolean) {
  const [kind, id] = [domain.slice(0, domain.indexOf(':')), domain.slice(domain.indexOf(':') + 1)]
  const what =
    kind === 'controller'
      ? `Shared with ${nodes.labelOf(id)}`
      : kind === 'ptp'
        ? `PTP domain ${id}`
        : 'This node only'
  return synchronized ? `${what}, synchronized start` : what
}


const confirmRemove = ref(false)
const confirmDeleteAll = ref(false)

const fileCount = computed(() => outputs.value.reduce((n, o) => n + o.files.length, 0))
const filesLabel = (n: number) => plural(n, 'file')

async function remove() {
  try {
    await recordings.remove(s.value.node_id, s.value.id)
    toast.success('Removed from history', { description: 'Its files are still on disk.' })
  } catch (e) {
    notifyError('Could not remove the recording', e, s.value.node_id)
  }
}

async function deleteAll() {
  const count = fileCount.value
  try {
    await recordings.remove(s.value.node_id, s.value.id, { files: true })
    toast.success(`Deleted ${filesLabel(count)}`, { description: 'The recording was removed from history.' })
  } catch (e) {
    notifyError('Could not delete every file. The recording stays, listing those left', e, s.value.node_id)
  }
}
</script>

<template>
  <section class="h-full min-h-0 flex bg-card">
    <div class="w-[40%] max-w-[560px] min-w-56 shrink-0 flex flex-col border-r border-border">
      <div class="bg-black aspect-video grid place-items-center">
        <video
          v-if="playingUrl && !playError"
          :ref="player.video"
          :key="playingUrl"
          :src="playingUrl"
          :autoplay="autoplay"
          class="w-full h-full"
          controls
          preload="metadata"
          @error="playError = true"
        />
        <p v-else class="px-6 text-center text-xs text-zinc-400">
          <template v-if="playingDeleted">This file was deleted.</template>
          <template v-else-if="playError">
            This file couldn't be played. It may have been moved or deleted, or still be open from a crash.
          </template>
          <template v-else>{{ noPreview }}</template>
        </p>
      </div>
      <div v-if="playingCaption" class="flex items-center gap-1.5 px-3 h-7 text-[11px]">
        <Play class="size-3 shrink-0 text-primary fill-current" />
        <span class="font-medium shrink-0">{{ playingCaption.output }}</span>
        <span class="num truncate text-muted-foreground" :title="playingCaption.file">{{ playingCaption.file }}</span>
        <span v-if="playingCaption.part" class="num shrink-0 text-muted-foreground">· {{ playingCaption.part }}</span>
      </div>
    </div>

    <div class="flex-1 min-w-0 flex flex-col">
      <div class="shrink-0 flex flex-wrap items-center gap-x-3 gap-y-1 min-h-10 px-3 py-1.5 border-b border-border">
        <h2 class="text-sm font-semibold truncate">{{ recordings.sourceNameOf(s) }}</h2>
        <span class="flex items-center gap-1.5 text-xs" :class="live ? 'text-tally font-medium' : 'text-muted-foreground'">
          <StatusDot :status="STATUS[s.status].dot" /> {{ STATUS[s.status].label }}
        </span>
        <span class="num text-xs" :class="live && 'text-tally'">{{ whenText }}</span>
        <span class="text-xs text-muted-foreground truncate">{{ nodes.nameOf(s.node_id) }} · {{ recordings.presetNameOf(s) }}</span>
        <div class="flex-1" />
        <div class="flex items-center gap-2">
          <Tooltip>
            <TooltipTrigger as-child>
              <!-- Wrapped so the tooltip still shows while the button is disabled. -->
              <span class="inline-block">
                <Button
                  variant="outline"
                  size="sm"
                  class="h-7 gap-1.5 text-xs hover:text-destructive hover:border-destructive/40"
                  :disabled="live"
                  @click="confirmRemove = true"
                >
                  <Trash2 class="size-3.5" /> Remove from history
                </Button>
              </span>
            </TooltipTrigger>
            <TooltipContent>
              {{ live ? 'Stop the recording first' : "Removes the session from this list. Its files stay on disk." }}
            </TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger as-child>
              <span class="inline-block">
                <Button variant="destructive" size="sm" class="h-7 gap-1.5 text-xs" :disabled="live" @click="confirmDeleteAll = true">
                  <Trash2 class="size-3.5" /> Delete files
                </Button>
              </span>
            </TooltipTrigger>
            <TooltipContent>
              {{ live ? 'Stop the recording first' : 'Deletes every file from disk and removes the session from history.' }}
            </TooltipContent>
          </Tooltip>
          <button class="icon-btn" title="Close (Esc)" @click="$emit('close')"><X class="size-3.5" /></button>
        </div>
      </div>

      <div class="flex-1 min-h-0 overflow-y-auto p-3 space-y-3">
        <p v-if="s.error_message" class="text-xs text-destructive break-words">{{ s.error_message }}</p>

        <RecordingOutputs
          :session="s"
          :outputs="outputs"
          :can-play="canPlay"
          :is-playing="isPlaying"
          @play="play"
        />

        <Collapsible v-model:open="detailsOpen">
          <CollapsibleTrigger class="flex items-center gap-1 section-title hover:text-foreground">
            <ChevronRight class="size-3.5 transition-transform" :class="detailsOpen && 'rotate-90'" />
            Details
          </CollapsibleTrigger>
          <CollapsibleContent>
            <div class="pt-1.5 max-w-md">
              <KeyValueList :items="details" />
            </div>
          </CollapsibleContent>
        </Collapsible>
      </div>
    </div>

    <ConfirmDialog
      v-model:open="confirmRemove"
      :title="`Remove ${recordings.sourceNameOf(s)}'s recording from history?`"
      description="The session disappears from Recordings on every client. Its files are not deleted and stay on disk."
      confirm-label="Remove"
      @confirm="remove"
    />
    <ConfirmDialog
      v-model:open="confirmDeleteAll"
      :title="`Delete ${recordings.sourceNameOf(s)}'s recording and its files?`"
      :description="`${filesLabel(fileCount)} on ${nodes.labelOf(s.node_id)} will be permanently deleted from disk, and the session removed from Recordings on every client. This can't be undone.`"
      :confirm-label="`Delete ${filesLabel(fileCount)}`"
      @confirm="deleteAll"
    />
  </section>
</template>

<style scoped>
@reference "@/style.css";
.icon-btn {
  @apply size-6 shrink-0 grid place-items-center rounded text-muted-foreground hover:text-foreground hover:bg-accent;
}
.section-title {
  @apply text-[11px] font-semibold uppercase tracking-wider text-muted-foreground;
}
</style>
