<script setup lang="ts">
import { computed } from 'vue'
import { RefreshCw } from '@lucide/vue'
import type { DeviceSourceConfig } from '@/types/generated/DeviceSourceConfig'
import type { DeviceDto } from '@/types/generated/DeviceDto'
import type { DeviceMode } from '@/types/generated/DeviceMode'
import { fpsLabel } from '@/lib/sourceFormat'
import { Button } from '@/components/ui/button'
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'
import AudioPlanField from './AudioPlanField.vue'

/** A capture device (camera, capture card, screen) and the audio paired with it. */
const props = defineProps<{
  devices: DeviceDto[]
  loading: boolean
  error: string | null
}>()
const emit = defineEmits<{ refresh: [] }>()

const cfg = defineModel<DeviceSourceConfig>({ required: true })

const video = computed(() => props.devices.filter((d) => d.kind !== 'audio'))
const current = computed(() => video.value.find((d) => d.key === cfg.value.video_device))

function modeLabel(m: DeviceMode) {
  const rate = m.fps_num ? ` ${fpsLabel([m.fps_num, m.fps_den])}` : ''
  return `${m.width}×${m.height}${rate}`
}

function best(d: DeviceDto) {
  const m = d.modes[0]
  return m ? ` — ${modeLabel(m)}` : ''
}

const deviceOptions = computed(() => {
  const opts = video.value.map((d) => ({
    value: d.key,
    label: `${d.kind === 'screen' ? 'Screen: ' : ''}${d.name}${best(d)}`,
  }))
  // A saved device that's unplugged stays visible, so the form doesn't
  // silently pick another.
  if (cfg.value.video_device && !current.value) {
    opts.unshift({
      value: cfg.value.video_device,
      label: `${cfg.value.video_device_name || cfg.value.video_device} (not connected)`,
    })
  }
  return opts
})

const deviceKey = computed({
  get: () => cfg.value.video_device,
  set: (key: string) => (cfg.value = { ...cfg.value, video_device: key, format: null }),
})

const DEFAULT = 'default'
const modeKey = (m: { width: number; height: number; fps_num: number; fps_den: number }) =>
  `${m.width}x${m.height}@${m.fps_num}/${m.fps_den}`

const modeOptions = computed(() => [
  { value: DEFAULT, label: "Device's default" },
  ...(current.value?.modes ?? [])
    .filter((m) => m.fps_num > 0)
    .map((m) => ({ value: modeKey(m), label: `${modeLabel(m)} · ${m.formats.join(', ')}` })),
])

const mode = computed({
  get: () => (cfg.value.format ? modeKey(cfg.value.format) : DEFAULT),
  set: (key: string) => {
    const m = current.value?.modes.find((m) => modeKey(m) === key)
    cfg.value = {
      ...cfg.value,
      format: m ? { width: m.width, height: m.height, fps_num: m.fps_num, fps_den: m.fps_den } : null,
    }
  },
})
</script>

<template>
  <div class="col-span-2 flex flex-col gap-3">
    <FormField label="Video device">
      <div class="flex gap-1.5">
        <OptionSelect v-model="deviceKey" :options="deviceOptions" :disabled="!deviceOptions.length" />
        <Button
          type="button"
          variant="outline"
          size="icon"
          class="shrink-0"
          title="Refresh devices"
          :disabled="loading"
          @click="emit('refresh')"
        >
          <RefreshCw class="size-3.5" :class="loading && 'animate-spin'" />
        </Button>
      </div>
    </FormField>
    <p v-if="error" class="-mt-1.5 text-xs text-destructive">{{ error }}</p>
    <p v-else-if="!loading && !video.length" class="-mt-1.5 text-xs text-muted-foreground">
      No cameras or capture cards on this node. Plug one in and refresh.
    </p>

    <FormField v-if="current" label="Capture format">
      <OptionSelect v-model="mode" :options="modeOptions" />
    </FormField>

    <AudioPlanField v-model="cfg.audio" :devices="devices" />
  </div>
</template>
