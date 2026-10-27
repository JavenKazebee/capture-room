<script setup lang="ts">
import { computed } from 'vue'
import type { DeviceDto } from '@/types/generated/DeviceDto'
import type { DisplayDto } from '@/types/generated/DisplayDto'
import type { DisplayIdle } from '@/types/generated/DisplayIdle'
import type { OutputConfig } from '@/types/generated/OutputConfig'
import type { VideoFormat } from '@/types/generated/VideoFormat'
import { hz, matchingMode } from '@/lib/displays'
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'
import OutputRow from './OutputRow.vue'

type DisplayOutput = Extract<OutputConfig, { type: 'display' }>

const props = defineProps<{
  output: DisplayOutput
  index: number
  /** The channel's format, which the screen is switched to if it can be. */
  format: VideoFormat
  /** The node's screens. */
  displays: DisplayDto[]
  /** The node's devices; its audio outputs are offered. */
  devices: DeviceDto[]
}>()
const emit = defineEmits<{ update: [patch: Partial<DisplayOutput>]; remove: [] }>()

const display = computed(() => props.displays.find((d) => d.id === props.output.display))

const displayOptions = computed(() => {
  const options = props.displays.map((d) => ({ value: d.id, label: `${d.name}${d.primary ? ' (main screen)' : ''}` }))
  const id = props.output.display
  return display.value || !id ? options : [{ value: id, label: id }, ...options]
})

/** What the screen will do with this channel's format. */
const hint = computed(() => {
  const d = display.value
  const id = props.output.display
  if (!d) return id ? `${id} isn't connected now; the output starts when it is.` : 'Pick a screen.'
  const m = matchingMode(d, props.format)
  if (m) return `The screen is set to ${m.width}×${m.height} at ${hz(m.refresh_mhz)} while this plays.`
  const { width, height } = props.format
  return `This screen has no ${width}×${height} mode at the channel's rate: it stays at ${d.width}×${d.height} at ${hz(d.refresh_mhz)} and the picture is scaled.`
})

/** "No audio": select items can't have an empty value. */
const NO_AUDIO = 'none'
const audioOptions = computed(() => [
  { value: NO_AUDIO, label: 'No audio' },
  ...props.devices.filter((d) => d.kind === 'output').map((d) => ({ value: d.key, label: d.name })),
])

const IDLE_OPTIONS: { value: DisplayIdle; label: string }[] = [
  { value: 'black', label: 'Black' },
  { value: 'desktop', label: 'The desktop (output hides)' },
]
</script>

<template>
  <OutputRow label="Screen" :index="index" @remove="emit('remove')">
    <template #control>
      <OptionSelect
        class="flex-1 min-w-0"
        :model-value="output.display"
        :options="displayOptions"
        :aria-label="`Screen for output ${index + 1}`"
        @update:model-value="(v) => emit('update', { display: String(v) })"
      />
    </template>

    <p class="text-xs text-muted-foreground">{{ hint }}</p>
    <p v-if="display?.primary" class="text-xs text-warning">
      This is the node's main screen. The output covers all of it, menu bar and Dock included, until the channel
      stops or the output is removed — from another device's browser if this is the only screen.
    </p>
    <FormField label="Audio">
      <OptionSelect
        :model-value="output.audio_device ?? NO_AUDIO"
        :options="audioOptions"
        @update:model-value="(v) => emit('update', { audio_device: v === NO_AUDIO ? null : String(v) })"
      />
    </FormField>
    <p class="text-xs text-muted-foreground">
      For an HDMI→SDI converter, pick the screen's own HDMI audio so it's embedded.
    </p>
    <FormField label="When nothing is loaded, show">
      <OptionSelect
        :model-value="output.idle"
        :options="IDLE_OPTIONS"
        @update:model-value="(v) => emit('update', { idle: v as DisplayIdle })"
      />
    </FormField>
    <p class="text-xs text-muted-foreground">
      <template v-if="output.idle === 'desktop'">
        The output appears when a clip is cued and hides when the channel stops. Not for converters: the signal
        drops while it's hidden.
      </template>
      <template v-else>Keeps the signal steady, which converters and switchers need.</template>
    </p>
  </OutputRow>
</template>
