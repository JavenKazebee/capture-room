<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { audioLevels, thumbnailSeqs, type Source } from '@/stores/sources'
import { useRecordingsStore, type RecordingSession } from '@/stores/recordings'
import { usePresetsStore } from '@/stores/presets'
import AudioMeter from './AudioMeter.vue'
import { Button } from '@/components/ui/button'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'

const props = defineProps<{
  source: Source
  session: RecordingSession | null
}>()

const recordings = useRecordingsStore()
const presets = usePresetsStore()

// ── Preset selection ──────────────────────────────────────────────────────────

// The built-in "default" is always available and resolves to H.264/MOV
// server-side even when no presets have been authored yet.
const presetOptions = computed(() => [
  { id: 'default', label: 'H.264 (default)' },
  ...presets.presets.map((p) => ({ id: p.id, label: p.name })),
])

const selectedPreset = ref('default')

// ── Thumbnail ─────────────────────────────────────────────────────────────────

const thumbError = ref(false)
const fallbackSeq = ref(0)

const thumbnailSrc = computed(() => {
  const wsSeq = thumbnailSeqs.get(props.source.id) ?? 0
  const seq = Math.max(wsSeq, fallbackSeq.value)
  return `/api/v1/thumbnails/${props.source.id}?t=${seq}`
})

// Reset error when a new thumbnail.updated WS event arrives
watch(
  () => thumbnailSeqs.get(props.source.id) ?? 0,
  (seq) => { if (seq > 0) thumbError.value = false },
)

// Retry on a slow interval — if thumbError is set, bump fallbackSeq so the
// URL changes and the browser doesn't serve a cached 404.
let retryTimer: ReturnType<typeof setInterval> | null = null
onMounted(() => {
  retryTimer = setInterval(() => {
    if (thumbError.value) {
      fallbackSeq.value++
      thumbError.value = false
    }
  }, 3000)
})
onUnmounted(() => {
  if (retryTimer) clearInterval(retryTimer)
})

// ── Audio ─────────────────────────────────────────────────────────────────────

const channels = computed(() => audioLevels.get(props.source.id) ?? [])

// ── Recording controls ────────────────────────────────────────────────────────

const busy = ref(false)

async function toggleRecording() {
  if (busy.value) return
  busy.value = true
  try {
    if (props.session) {
      await recordings.stop(props.session.id)
    } else {
      await recordings.start(props.source.id, selectedPreset.value)
    }
  } finally {
    busy.value = false
  }
}

// ── Duration ─────────────────────────────────────────────────────────────────

function formatDuration(startedAt: string): string {
  const elapsed = Math.floor((Date.now() - new Date(startedAt).getTime()) / 1000)
  const h = Math.floor(elapsed / 3600)
  const m = Math.floor((elapsed % 3600) / 60)
  const s = elapsed % 60
  return h > 0
    ? `${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`
    : `${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`
}
</script>

<template>
  <div
    class="rounded-lg border border-border bg-card overflow-hidden flex flex-col"
    :class="{ 'opacity-60': !source.is_available }"
  >
    <!-- Thumbnail + meters row -->
    <div class="relative flex bg-black" style="aspect-ratio: 16/9">
      <!-- Thumbnail -->
      <div class="flex-1 relative overflow-hidden">
        <img
          v-if="!thumbError"
          :src="thumbnailSrc"
          :alt="source.display_name"
          class="w-full h-full object-cover"
          @error="thumbError = true"
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
          {{ source.timecode.display }}
        </div>

        <!-- Recording indicator + duration (top-right) -->
        <div
          v-if="session"
          class="absolute top-1.5 right-1.5 flex items-center gap-1.5 bg-black/70 px-1.5 py-0.5 rounded"
        >
          <span class="w-2 h-2 rounded-full bg-red-500 animate-pulse shrink-0" />
          <span class="text-white text-[10px] font-mono">
            {{ formatDuration(session.started_at) }}
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
          {{ source.source_type }}
        </span>
      </div>

      <!-- Controls row -->
      <div class="flex gap-2 items-center">
        <Select
          v-model="selectedPreset"
          :disabled="!!session || busy"
        >
          <SelectTrigger class="h-7 text-xs flex-1">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem
              v-for="preset in presetOptions"
              :key="preset.id"
              :value="preset.id"
              class="text-xs"
            >
              {{ preset.label }}
            </SelectItem>
          </SelectContent>
        </Select>

        <Button
          :variant="session ? 'destructive' : 'default'"
          size="sm"
          class="h-7 px-3 text-xs shrink-0"
          :disabled="busy || !source.is_available"
          @click="toggleRecording"
        >
          {{ session ? 'Stop' : 'Record' }}
        </Button>
      </div>
    </div>
  </div>
</template>
