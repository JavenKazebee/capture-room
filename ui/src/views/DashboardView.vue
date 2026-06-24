<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useSourcesStore, type Source } from '@/stores/sources'
import { useRecordingsStore } from '@/stores/recordings'
import { usePresetsStore } from '@/stores/presets'
import { wsStatus } from '@/composables/useWebSocket'
import { useApi } from '@/composables/useApi'
import FeedCard from '@/components/FeedCard.vue'
import { Button } from '@/components/ui/button'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { WifiOff } from '@lucide/vue'

const sources = useSourcesStore()
const recordings = useRecordingsStore()
const presets = usePresetsStore()
const { api } = useApi()

// ── Monitor settings ──────────────────────────────────────────────────────────

interface MonitorSettings {
  thumb_fps: number
  thumb_width: number
  thumb_height: number
  level_interval_ms: number
}

const monitor = ref<MonitorSettings>({
  thumb_fps: 1,
  thumb_width: 320,
  thumb_height: 180,
  level_interval_ms: 100,
})
const monitorSaving = ref(false)

const thumbSizeOptions = [
  { label: '320×180', width: 320, height: 180 },
  { label: '640×360', width: 640, height: 360 },
  { label: '1280×720', width: 1280, height: 720 },
]

const thumbFpsOptions = [
  { label: '1 fps', value: 1 },
  { label: '2 fps', value: 2 },
  { label: '5 fps', value: 5 },
  { label: '10 fps', value: 10 },
]

const levelIntervalOptions = [
  { label: '50 ms', value: 50 },
  { label: '100 ms', value: 100 },
  { label: '200 ms', value: 200 },
  { label: '500 ms', value: 500 },
]

const thumbSizeValue = ref('320x180')
const thumbFpsValue = ref('1')
const levelIntervalValue = ref('100')

function thumbSizeKey(w: number, h: number) {
  return `${w}x${h}`
}

async function saveMonitorSettings() {
  if (monitorSaving.value) return
  monitorSaving.value = true
  const size = thumbSizeOptions.find(o => thumbSizeKey(o.width, o.height) === thumbSizeValue.value)
  try {
    const updated = await api<MonitorSettings>('/settings/monitor', {
      method: 'PUT',
      body: {
        thumb_fps: Number(thumbFpsValue.value),
        thumb_width: size?.width ?? monitor.value.thumb_width,
        thumb_height: size?.height ?? monitor.value.thumb_height,
        level_interval_ms: Number(levelIntervalValue.value),
      },
    })
    monitor.value = updated
  } finally {
    monitorSaving.value = false
  }
}

// ── Load ──────────────────────────────────────────────────────────────────────

onMounted(async () => {
  presets.load()
  const [fetchedSources, fetchedRecordings, settings] = await Promise.all([
    api('/sources').catch(() => []),
    api('/recordings').catch(() => []),
    api<{ monitor: MonitorSettings }>('/settings').catch(() => null),
  ])

  for (const s of fetchedSources as Source[]) {
    sources.upsert(s)
  }

  for (const r of fetchedRecordings as Parameters<typeof recordings.upsert>[0][]) {
    recordings.upsert(r)
  }

  if (settings?.monitor) {
    monitor.value = settings.monitor
    thumbSizeValue.value = thumbSizeKey(settings.monitor.thumb_width, settings.monitor.thumb_height)
    thumbFpsValue.value = String(settings.monitor.thumb_fps)
    levelIntervalValue.value = String(settings.monitor.level_interval_ms)
  }
})
</script>

<template>
  <div class="flex flex-col h-full">
    <!-- Reconnecting banner -->
    <div
      v-if="wsStatus !== 'connected'"
      class="flex items-center gap-2 px-4 py-2 bg-yellow-500/15 border-b border-yellow-500/30 text-yellow-700 dark:text-yellow-400 text-sm"
    >
      <WifiOff class="w-4 h-4 shrink-0" />
      <span>
        {{
          wsStatus === 'connecting'
            ? 'Connecting to server…'
            : 'Disconnected — reconnecting…'
        }}
      </span>
    </div>

    <!-- Main content -->
    <div class="flex-1 overflow-y-auto p-6">
      <!-- Title + monitor settings row -->
      <div class="flex items-center justify-between mb-6 gap-4">
        <h1 class="text-2xl font-semibold shrink-0">Dashboard</h1>

        <!-- Monitor settings -->
        <div class="flex items-center gap-2 flex-wrap">
          <span class="text-xs text-muted-foreground shrink-0">Thumbnail</span>
          <Select v-model="thumbSizeValue" :disabled="monitorSaving">
            <SelectTrigger class="h-7 text-xs w-28">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem
                v-for="opt in thumbSizeOptions"
                :key="opt.label"
                :value="thumbSizeKey(opt.width, opt.height)"
                class="text-xs"
              >{{ opt.label }}</SelectItem>
            </SelectContent>
          </Select>

          <Select v-model="thumbFpsValue" :disabled="monitorSaving">
            <SelectTrigger class="h-7 text-xs w-20">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem
                v-for="opt in thumbFpsOptions"
                :key="opt.value"
                :value="String(opt.value)"
                class="text-xs"
              >{{ opt.label }}</SelectItem>
            </SelectContent>
          </Select>

          <span class="text-xs text-muted-foreground shrink-0">Audio</span>
          <Select v-model="levelIntervalValue" :disabled="monitorSaving">
            <SelectTrigger class="h-7 text-xs w-20">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem
                v-for="opt in levelIntervalOptions"
                :key="opt.value"
                :value="String(opt.value)"
                class="text-xs"
              >{{ opt.label }}</SelectItem>
            </SelectContent>
          </Select>

          <Button
            size="sm"
            variant="outline"
            class="h-7 px-3 text-xs"
            :disabled="monitorSaving"
            @click="saveMonitorSettings"
          >
            {{ monitorSaving ? 'Applying…' : 'Apply' }}
          </Button>
        </div>
      </div>

      <!-- Empty state -->
      <div
        v-if="sources.sources.length === 0"
        class="text-center text-muted-foreground py-24"
      >
        <p class="text-lg font-medium mb-1">No sources found</p>
        <p class="text-sm">Make sure the capture node is running and has sources available.</p>
      </div>

      <!-- Feed grid -->
      <div
        v-else
        class="grid gap-4"
        :class="wsStatus !== 'connected' ? 'opacity-60 pointer-events-none' : ''"
        style="grid-template-columns: repeat(3, minmax(0, 1fr))"
      >
        <FeedCard
          v-for="source in sources.sources"
          :key="source.id"
          :source="source"
          :session="recordings.activeForSource(source.id)"
        />
      </div>
    </div>
  </div>
</template>
