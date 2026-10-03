<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useNow } from '@vueuse/core'
import { audioLevels, thumbnailSeqs, type Source } from '@/stores/sources'
import { useRecordingsStore, type RecordingSession } from '@/stores/recordings'
import { usePresetsStore } from '@/stores/presets'
import { errorMessage, thumbnailUrl } from '@/composables/useApi'
import { useNodesStore } from '@/stores/nodes'
import AudioMeter from './AudioMeter.vue'
import { Button } from '@/components/ui/button'
import OptionSelect from './OptionSelect.vue'

const props = defineProps<{
  source: Source
  session: RecordingSession | null
}>()

const recordings = useRecordingsStore()
const presets = usePresetsStore()
const nodes = useNodesStore()

// ── Preset selection ──────────────────────────────────────────────────────────

// The built-in "default" is always available: a single H.264/MOV output,
// usable even when no presets have been authored yet.
const presetOptions = computed(() => [
  { value: 'default', label: 'H.264 (default)' },
  ...presets.presets.map((p) => ({ value: p.id, label: p.name })),
])

const selectedPreset = ref('default')

// ── Thumbnail ─────────────────────────────────────────────────────────────────

// Bumped by every `thumbnail.updated` event, which only monitored sources get
// (at least once a second). A failed load is retried by the next bump.
const thumbSeq = computed(() => thumbnailSeqs.get(props.source.key) ?? 0)
const thumbFailed = ref(false)
watch(thumbSeq, () => (thumbFailed.value = false))

const thumbnailSrc = computed(
  () => `${thumbnailUrl(props.source.node_id, props.source.id)}?t=${thumbSeq.value}`,
)

// ── Audio ─────────────────────────────────────────────────────────────────────

const channels = computed(() => audioLevels.get(props.source.key) ?? [])

// ── Recording controls ────────────────────────────────────────────────────────

const busy = ref(false)
const actionError = ref<string | null>(null)

async function toggleRecording() {
  if (busy.value) return
  busy.value = true
  actionError.value = null
  try {
    if (props.session) {
      await recordings.stop(props.source.node_id, props.session.id)
    } else {
      const preset = presets.presets.find((p) => p.id === selectedPreset.value) ?? null
      await recordings.start(props.source.node_id, props.source.id, preset)
    }
  } catch (e) {
    actionError.value = errorMessage(e, props.session ? 'Stop failed.' : 'Record failed.')
  } finally {
    busy.value = false
  }
}

// When a session ends — stopped here, or on its own because every output or
// the source failed — show why if it ended in error.
watch(
  () => props.session,
  (now, prev) => {
    if (now || !prev) return
    const ended = recordings.find(prev.node_id, prev.id)
    if (ended?.status === 'error') actionError.value = ended.error_message
  },
)

// ── Duration ─────────────────────────────────────────────────────────────────

// Ticks once a second so the timer advances on its own (it used to update
// only when audio levels happened to re-render the card).
const now = useNow({ interval: 1000 })

const duration = computed(() =>
  props.session ? formatDuration(now.value.getTime() - new Date(props.session.started_at).getTime()) : '',
)

function formatDuration(ms: number): string {
  const elapsed = Math.max(0, Math.floor(ms / 1000))
  const h = Math.floor(elapsed / 3600)
  const m = Math.floor((elapsed % 3600) / 60)
  const s = elapsed % 60
  return h > 0
    ? `${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`
    : `${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`
}
</script>

<template>
  <div class="rounded-lg border border-border bg-card overflow-hidden flex flex-col">
    <!-- Thumbnail + meters row -->
    <div class="relative flex bg-black" style="aspect-ratio: 16/9">
      <!-- Thumbnail -->
      <div class="flex-1 relative overflow-hidden">
        <div
          v-if="source.error"
          class="w-full h-full flex flex-col items-center justify-center gap-1 px-4 text-center text-xs"
        >
          <span class="text-destructive font-medium">Source failed</span>
          <span class="text-muted-foreground line-clamp-3">{{ source.error }}</span>
        </div>
        <img
          v-else-if="thumbSeq > 0 && !thumbFailed"
          :src="thumbnailSrc"
          :alt="source.display_name"
          class="w-full h-full object-cover"
          @error="thumbFailed = true"
        />
        <div
          v-else
          class="w-full h-full flex items-center justify-center text-muted-foreground text-xs"
        >
          No signal
        </div>

        <!-- Timecode overlay (bottom-left) -->
        <div
          v-if="source.timecode"
          class="absolute bottom-1.5 left-1.5 bg-black/70 text-white text-[10px] font-mono px-1.5 py-0.5 rounded"
        >
          {{ source.timecode }}
        </div>

        <!-- Recording indicator + duration (top-right) -->
        <div
          v-if="session"
          class="absolute top-1.5 right-1.5 flex items-center gap-1.5 bg-black/70 px-1.5 py-0.5 rounded"
        >
          <span class="w-2 h-2 rounded-full bg-red-500 animate-pulse shrink-0" />
          <span class="text-white text-[10px] font-mono">
            {{ duration }}
          </span>
        </div>
      </div>

      <!-- Audio meters (right edge) -->
      <div v-if="channels.length > 0" class="w-8 py-1 shrink-0">
        <AudioMeter :channels="channels" />
      </div>
    </div>

    <!-- Info + controls -->
    <div class="px-3 py-2 flex flex-col gap-2">
      <!-- Source name + type -->
      <div class="flex items-center justify-between">
        <span class="text-sm font-medium truncate">{{ source.display_name }}</span>
        <span class="text-[10px] text-muted-foreground uppercase tracking-wide shrink-0 ml-2">
          <template v-if="nodes.nodes.length > 1">{{ nodes.nameOf(source.node_id) }} · </template>{{ source.source_type }}
        </span>
      </div>

      <!-- Controls row -->
      <div class="flex gap-2 items-center">
        <OptionSelect
          v-model="selectedPreset"
          :options="presetOptions"
          :disabled="!!session || busy"
          class="h-7 text-xs flex-1 min-w-0"
        />

        <Button
          :variant="session ? 'destructive' : 'default'"
          size="sm"
          class="h-7 px-3 text-xs shrink-0"
          :disabled="busy"
          @click="toggleRecording"
        >
          {{ session ? 'Stop' : 'Record' }}
        </Button>
      </div>

      <p v-if="session?.error_message" class="text-xs text-destructive break-words">
        Output failed: {{ session.error_message }}
      </p>
      <p v-if="actionError" class="text-xs text-destructive break-words">{{ actionError }}</p>
    </div>
  </div>
</template>
