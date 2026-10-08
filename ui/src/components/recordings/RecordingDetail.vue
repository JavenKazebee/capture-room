<script setup lang="ts">
import { computed, ref } from 'vue'
import { useClipboard, useNow, useStorage } from '@vueuse/core'
import { toast } from 'vue-sonner'
import { ChevronRight, Download, ListPlus, Play, Share2, Trash2, X } from '@lucide/vue'
import { useRouter } from 'vue-router'
import { recordingFileUrl } from '@/composables/useApi'
import { formatDuration, formatTimeRange } from '@/lib/format'
import { CODECS, CONTAINERS } from '@/lib/codecs'
import { notifyError } from '@/lib/notify'
import { useNodesStore } from '@/stores/nodes'
import { outputFiles, previewOutput, useRecordingsStore, type RecordingSession } from '@/stores/recordings'
import { usePlayoutStore } from '@/stores/playout'
import type { SessionAudio } from '@/types/generated/SessionAudio'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import CopyButton from '@/components/common/CopyButton.vue'
import KeyValueList from '@/components/common/KeyValueList.vue'
import StatusDot from '@/components/common/StatusDot.vue'

const props = defineProps<{ session: RecordingSession }>()
defineEmits<{ close: [] }>()

const recordings = useRecordingsStore()
const nodes = useNodesStore()
const now = useNow({ interval: 1000 })
const clipboard = useClipboard({ legacy: true })

const s = computed(() => props.session)
const live = computed(() => s.value.status === 'active')

// ── Player ────────────────────────────────────────────────────────────────────

/**
 * The file playing: an output and one of its files (a split output has
 * several), by path, since deleting a file shifts the indexes after it.
 */
const playing = ref<{ output: number; path: string } | null>(initialPlaying())
function initialPlaying() {
  const output = previewOutput(props.session)
  const path = output === -1 ? undefined : outputFiles(props.session, output)[0]
  return path === undefined ? null : { output, path }
}
/** The playing file's index in its output, `-1` once it's been deleted. */
const playingFile = computed(() =>
  playing.value ? (outputs.value[playing.value.output]?.files.indexOf(playing.value.path) ?? -1) : -1,
)
const playingDeleted = computed(() => !!playing.value && playingFile.value === -1)
const playError = ref(false)
const video = ref<HTMLVideoElement>()
/** Starts playback as soon as a file loads — only once someone picks a file, not on open. */
const autoplay = ref(false)

const playingUrl = computed(() =>
  playing.value && !live.value && playingFile.value !== -1
    ? recordingFileUrl(s.value.node_id, s.value.id, playing.value.output, playingFile.value)
    : null,
)

/** What the player shows, for the caption under it. */
const playingCaption = computed(() => {
  if (!playingUrl.value || !playing.value) return null
  const o = outputs.value[playing.value.output]
  if (!o) return null
  return {
    output: o.name,
    file: fileName(playing.value.path),
    part: o.files.length > 1 ? `${playingFile.value + 1} of ${o.files.length}` : null,
  }
})

function play(output: number, file: number) {
  if (!canPlay(output)) return
  if (isPlaying(output, file) && video.value && !playError.value) {
    // Already loaded: play it from the start.
    video.value.currentTime = 0
    void video.value.play()
    return
  }
  const path = outputs.value[output]?.files[file]
  if (path === undefined) return
  playing.value = { output, path }
  playError.value = false
  autoplay.value = true
}

const isPlaying = (output: number, file: number) => !live.value && playing.value?.output === output && playingFile.value === file

const canPlay = (output: number) => !live.value && !!s.value.outputs[output]?.playable

/** Plays or pauses the loaded file, or starts the preview file when nothing's loaded yet. */
function togglePlay() {
  if (video.value && !playError.value) {
    if (video.value.paused) void video.value.play()
    else video.value.pause()
    return
  }
  const output = previewOutput(s.value)
  if (output !== -1) play(output, 0)
}
defineExpose({ togglePlay })

// ── Share ─────────────────────────────────────────────────────────────────────

const playout = usePlayoutStore()
const router = useRouter()

/** Adds a file to its node's media library, for playing out from Playback. */
async function addToMedia(path: string) {
  try {
    const item = await playout.addMedia(s.value.node_id, path, s.value.id)
    toast.success(`Added ${item.name} to the media library`, {
      action: { label: 'Open Playback', onClick: () => router.push('/playback') },
    })
  } catch (e) {
    notifyError('Could not add the file to the media library', e, s.value.node_id)
  }
}

/** Shares a file's download link through the OS share sheet, or copies it where there isn't one. */
async function share(output: number, file: number, path: string) {
  const url = new URL(recordingFileUrl(s.value.node_id, s.value.id, output, file, true), window.location.origin).href
  if (navigator.share) {
    try {
      await navigator.share({ title: fileName(path), url })
      return
    } catch (e) {
      if (e instanceof DOMException && e.name === 'AbortError') return
      // Fall through to copying.
    }
  }
  await clipboard.copy(url)
  if (clipboard.copied.value) toast.success('Download link copied', { description: 'Anyone who can reach this Capture Room can open it.' })
  else toast.error('Could not copy the link', { description: url })
}

/** Why there's no preview, when there isn't one. */
const noPreview = computed(() => {
  if (live.value) return 'Still recording. Files can be previewed once the recording stops.'
  if (!s.value.outputs.length) return 'Recorded before previews were available, so its formats are unknown. Download a file to play it.'
  if (previewOutput(s.value) === -1)
    return "None of this session's outputs play in a browser. Add an H.264 output with AAC audio in .mp4 or .mov to its preset to preview recordings."
  return null
})

// ── Details ───────────────────────────────────────────────────────────────────

function when(iso: string | null) {
  return iso
    ? new Date(iso).toLocaleString([], { hour12: false, year: 'numeric', month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', second: '2-digit' })
    : null
}

const duration = computed(() => {
  const end = s.value.stopped_at ? new Date(s.value.stopped_at).getTime() : now.value.getTime()
  return formatDuration(end - new Date(s.value.started_at).getTime())
})

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
  if (!iso) return null
  const d = new Date(iso)
  const ms = String(d.getMilliseconds()).padStart(3, '0')
  return `${d.toLocaleTimeString([], { hour12: false })}.${ms}`
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

const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path

// ── Remove / delete ───────────────────────────────────────────────────────────

const confirmRemove = ref(false)
const confirmDeleteAll = ref(false)

const fileCount = computed(() => outputs.value.reduce((n, o) => n + o.files.length, 0))
const filesLabel = (n: number) => `${n} file${n === 1 ? '' : 's'}`

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

/** The single file awaiting confirmation to delete. */
const deleting = ref<{ output: number; file: number; path: string } | null>(null)
const confirmDeleteFile = computed({
  get: () => deleting.value !== null,
  set: (open) => {
    if (!open) deleting.value = null
  },
})

async function deleteFile() {
  const d = deleting.value
  if (!d) return
  try {
    await recordings.deleteFile(s.value.node_id, s.value.id, d.output, d.file, d.path)
    toast.success(`Deleted ${fileName(d.path)}`)
  } catch (e) {
    notifyError(`Could not delete ${fileName(d.path)}`, e, s.value.node_id)
  }
}
</script>

<template>
  <section class="h-full min-h-0 flex bg-card">
    <!-- Player -->
    <div class="w-[40%] max-w-[560px] min-w-56 shrink-0 flex flex-col border-r border-border">
      <div class="bg-black aspect-video grid place-items-center">
        <video
          v-if="playingUrl && !playError"
          ref="video"
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
      <!-- Summary: what, state, when, where, and what you can do with it -->
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

        <section class="space-y-1.5">
          <h3 class="section-title">Outputs</h3>
          <div v-for="(o, i) in outputs" :key="i" class="rounded-md border border-border">
            <div class="flex items-baseline gap-2 px-2.5 py-1.5 border-b border-border/60">
              <span class="text-xs font-medium">{{ o.name }}</span>
              <span v-if="o.format" class="text-[11px] text-muted-foreground truncate">{{ o.format }}</span>
              <div class="flex-1" />
              <span v-if="o.dropped" class="num text-[11px] text-warning shrink-0" title="Video frames dropped because this output's encoder couldn't keep up">
                {{ o.dropped.toLocaleString() }} dropped
              </span>
            </div>
            <ul class="divide-y divide-border/60">
              <li
                v-for="(f, j) in o.files"
                :key="f"
                class="flex items-center gap-1 px-2.5 py-1 text-xs"
                :class="[
                  canPlay(i) && 'cursor-pointer hover:bg-accent/40',
                  isPlaying(i, j) && 'bg-accent/60',
                ]"
                :role="canPlay(i) ? 'button' : undefined"
                :tabindex="canPlay(i) ? 0 : undefined"
                :aria-label="canPlay(i) ? `Play ${fileName(f)}` : undefined"
                :aria-current="isPlaying(i, j) || undefined"
                :title="!live && !canPlay(i) ? 'This format can\'t play in a browser. Download it to watch.' : undefined"
                @click="play(i, j)"
                @keydown.enter.self.prevent="play(i, j)"
                @keydown.space.self.prevent="play(i, j)"
              >
                <span class="w-3.5 shrink-0 grid place-items-center">
                  <Play v-if="isPlaying(i, j)" class="size-3 text-primary fill-current" />
                  <Play v-else-if="canPlay(i)" class="size-3 text-muted-foreground/50" />
                </span>
                <span
                  class="num truncate flex-1"
                  :class="isPlaying(i, j) ? 'text-foreground' : canPlay(i) || live ? 'text-muted-foreground' : 'text-muted-foreground/60'"
                  :title="f"
                >
                  {{ fileName(f) }}
                </span>
                <span v-if="isPlaying(i, j)" class="text-[10px] font-medium uppercase tracking-wider text-primary shrink-0 mr-1">Playing</span>
                <CopyButton :value="f" size="md" :label="`Copy path on ${nodes.labelOf(s.node_id)}`" />
                <Tooltip v-if="!live">
                  <TooltipTrigger as-child>
                    <button class="icon-btn" :aria-label="`Add ${fileName(f)} to the media library`" @click.stop="addToMedia(f)"><ListPlus class="size-3.5" /></button>
                  </TooltipTrigger>
                  <TooltipContent>Add to media library (for Playback)</TooltipContent>
                </Tooltip>
                <Tooltip v-if="!live">
                  <TooltipTrigger as-child>
                    <button class="icon-btn" :aria-label="`Share ${fileName(f)}`" @click.stop="share(i, j, f)"><Share2 class="size-3.5" /></button>
                  </TooltipTrigger>
                  <TooltipContent>Share download link</TooltipContent>
                </Tooltip>
                <Tooltip v-if="!live">
                  <TooltipTrigger as-child>
                    <a
                      class="icon-btn"
                      :href="recordingFileUrl(s.node_id, s.id, i, j, true)"
                      :aria-label="`Download ${fileName(f)}`"
                      download
                      @click.stop
                    >
                      <Download class="size-3.5" />
                    </a>
                  </TooltipTrigger>
                  <TooltipContent>Download</TooltipContent>
                </Tooltip>
                <Tooltip v-if="!live">
                  <TooltipTrigger as-child>
                    <button
                      class="icon-btn hover:text-destructive!"
                      :aria-label="`Delete ${fileName(f)}`"
                      @click.stop="deleting = { output: i, file: j, path: f }"
                    >
                      <Trash2 class="size-3.5" />
                    </button>
                  </TooltipTrigger>
                  <TooltipContent>Delete file</TooltipContent>
                </Tooltip>
              </li>
              <li v-if="!o.files.length" class="px-2.5 py-1 text-xs italic text-muted-foreground/60">No files</li>
            </ul>
          </div>
        </section>

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
    <ConfirmDialog
      v-model:open="confirmDeleteFile"
      :title="`Delete ${deleting ? fileName(deleting.path) : ''}?`"
      :description="`The file will be permanently deleted from ${nodes.labelOf(s.node_id)}'s disk. This can't be undone. The session stays in history.`"
      confirm-label="Delete file"
      @confirm="deleteFile"
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
