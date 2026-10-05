<script setup lang="ts">
import { computed, ref } from 'vue'
import { useNow } from '@vueuse/core'
import { toast } from 'vue-sonner'
import { Download, Play, Trash2, X } from '@lucide/vue'
import { recordingFileUrl } from '@/composables/useApi'
import { formatDuration } from '@/lib/format'
import { CODECS, CONTAINERS } from '@/lib/codecs'
import { notifyError } from '@/lib/notify'
import { useNodesStore } from '@/stores/nodes'
import { outputFiles, previewOutput, useRecordingsStore, type RecordingSession } from '@/stores/recordings'
import type { SessionAudio } from '@/types/generated/SessionAudio'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import CopyButton from '@/components/common/CopyButton.vue'
import KeyValueList from '@/components/common/KeyValueList.vue'

const props = defineProps<{ session: RecordingSession }>()
defineEmits<{ close: [] }>()

const recordings = useRecordingsStore()
const nodes = useNodesStore()
const now = useNow({ interval: 1000 })

const s = computed(() => props.session)
const live = computed(() => s.value.status === 'active')

// ── Player ────────────────────────────────────────────────────────────────────

/** The file playing: an output and one of its files (a split output has several). */
const playing = ref<{ output: number; file: number } | null>(
  previewOutput(props.session) === -1 ? null : { output: previewOutput(props.session), file: 0 },
)
const playError = ref(false)

const playingUrl = computed(() =>
  playing.value && !live.value ? recordingFileUrl(s.value.node_id, s.value.id, playing.value.output, playing.value.file) : null,
)

function play(output: number, file: number) {
  playing.value = { output, file }
  playError.value = false
}

const canPlay = (output: number) => !live.value && !!s.value.outputs[output]?.playable

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

const STATUS_LABEL = { active: 'Recording', stopped: 'Finished', error: 'Failed' } as const

const details = computed(() => [
  { label: 'Status', value: STATUS_LABEL[s.value.status] },
  { label: 'Node', value: nodes.labelOf(s.value.node_id) },
  { label: 'Preset', value: recordings.presetNameOf(s.value) },
  { label: 'Started', value: when(s.value.started_at), mono: true },
  { label: 'Stopped', value: when(s.value.stopped_at), mono: true },
  { label: 'Duration', value: duration.value, mono: true },
  { label: 'Session', value: s.value.id, mono: true, copy: true },
])

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

// ── Remove ────────────────────────────────────────────────────────────────────

const confirmRemove = ref(false)

async function remove() {
  try {
    await recordings.remove(s.value.node_id, s.value.id)
    toast.success('Removed from history', { description: 'Its files are still on disk.' })
  } catch (e) {
    notifyError('Could not remove the recording', e, s.value.node_id)
  }
}
</script>

<template>
  <aside class="h-full flex flex-col min-h-0 bg-card border-l border-border">
    <div class="h-9 shrink-0 flex items-center gap-2 px-3 border-b border-border">
      <span class="text-xs font-semibold truncate">{{ recordings.sourceNameOf(s) }}</span>
      <div class="flex-1" />
      <button class="icon-btn" title="Close" @click="$emit('close')"><X class="size-3.5" /></button>
    </div>

    <div class="flex-1 min-h-0 overflow-y-auto">
      <!-- Preview -->
      <div class="bg-black aspect-video grid place-items-center">
        <video
          v-if="playingUrl && !playError"
          :key="playingUrl"
          :src="playingUrl"
          class="w-full h-full"
          controls
          preload="metadata"
          @error="playError = true"
        />
        <p v-else class="px-6 text-center text-xs text-zinc-400">
          <template v-if="playError">
            This file couldn't be played. It may have been moved or deleted, or still be open from a crash.
          </template>
          <template v-else>{{ noPreview }}</template>
        </p>
      </div>

      <div class="p-3 space-y-4">
        <p v-if="s.error_message" class="text-xs text-destructive break-words">{{ s.error_message }}</p>

        <KeyValueList :items="details" />

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
                :class="playing?.output === i && playing.file === j && !live && 'bg-accent/50'"
              >
                <span class="num truncate flex-1 text-muted-foreground" :title="f">{{ fileName(f) }}</span>
                <CopyButton :value="f" />
                <Tooltip v-if="canPlay(i)">
                  <TooltipTrigger as-child>
                    <button class="icon-btn" :aria-label="`Play ${fileName(f)}`" @click="play(i, j)"><Play class="size-3.5" /></button>
                  </TooltipTrigger>
                  <TooltipContent>Play</TooltipContent>
                </Tooltip>
                <Tooltip v-if="!live">
                  <TooltipTrigger as-child>
                    <a
                      class="icon-btn"
                      :href="recordingFileUrl(s.node_id, s.id, i, j, true)"
                      :aria-label="`Download ${fileName(f)}`"
                      download
                    >
                      <Download class="size-3.5" />
                    </a>
                  </TooltipTrigger>
                  <TooltipContent>Download</TooltipContent>
                </Tooltip>
              </li>
            </ul>
          </div>
        </section>

        <div class="pt-2 border-t border-border">
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
        </div>
      </div>
    </div>

    <ConfirmDialog
      v-model:open="confirmRemove"
      :title="`Remove ${recordings.sourceNameOf(s)}'s recording from history?`"
      description="The session disappears from Recordings on every client. Its files are not deleted and stay on disk."
      confirm-label="Remove"
      @confirm="remove"
    />
  </aside>
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
