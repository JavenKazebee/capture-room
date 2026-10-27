<script setup lang="ts">
import { computed } from 'vue'
import type { DecklinkStatusDto } from '@/types/generated/DecklinkStatusDto'
import type { OutputConfig } from '@/types/generated/OutputConfig'
import type { VideoFormat } from '@/types/generated/VideoFormat'
import { findSubDevice, subDeviceOptions } from '@/lib/decklink'
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'
import DecklinkProfileField from '../DecklinkProfileField.vue'
import OutputRow from './OutputRow.vue'

type SdiOutput = Extract<OutputConfig, { type: 'decklink' }>

const props = defineProps<{
  output: SdiOutput
  index: number
  format: VideoFormat
  audioChannels: number
  /** The node's Decklink cards. */
  decklink: DecklinkStatusDto
  nodeId: string
  /** The channel being edited, so its own SDI outputs aren't flagged as taken. */
  channelId: string | null
}>()
const emit = defineEmits<{ update: [patch: Partial<SdiOutput>]; remove: []; refreshDecklink: [] }>()

const sub = computed(() => findSubDevice(props.decklink, props.output.device))

const deviceOptions = computed(() => {
  const options = subDeviceOptions(props.decklink, true, props.channelId)
  const key = props.output.device
  return !key || sub.value ? options : [{ value: key, label: `${key} (not connected)` }, ...options]
})

const MATCH = 'match'

/**
 * The modes the sub-device can play this channel in, at its size and rate. Mirrors `output_mode` in
 * `sources/decklink.rs`: of the interlaced modes only 1080i, whose field rate is the channel's frame rate.
 */
const modeOptions = computed(() => {
  const { width, height, fps_num, fps_den } = props.format
  const modes = (sub.value?.device.modes ?? []).filter((m) => {
    if (m.interlaced && m.height !== 1080) return false
    const fields = m.interlaced ? 2 : 1
    return m.width === width && m.height === height && m.fps_num * fields * fps_den === fps_num * m.fps_den
  })
  return [{ value: MATCH, label: "The channel's format" }, ...modes.map((m) => ({ value: m.mode, label: m.label }))]
})
</script>

<template>
  <OutputRow label="SDI" :index="index" @remove="emit('remove')">
    <template #control>
      <OptionSelect
        class="flex-1 min-w-0"
        :model-value="output.device"
        :options="deviceOptions"
        :aria-label="`Decklink output for output ${index + 1}`"
        @update:model-value="(v) => emit('update', { device: String(v), mode: null })"
      />
    </template>

    <FormField label="Video mode">
      <OptionSelect
        :model-value="output.mode ?? MATCH"
        :options="modeOptions"
        @update:model-value="(v) => emit('update', { mode: v === MATCH ? null : String(v) })"
      />
    </FormField>
    <p class="text-xs text-muted-foreground">
      The channel's format has to be an SDI mode. A 1080i mode plays each of the channel's frames as a
      field, so a 1080p50 channel can go out as 1080i50. Audio is embedded on
      {{ audioChannels <= 2 ? 2 : 8 }} channels. The card follows the node's clock, so it may repeat or drop
      a frame every few minutes.
    </p>
    <DecklinkProfileField v-if="sub" :node-id="nodeId" :card="sub.card" @changed="emit('refreshDecklink')" />
  </OutputRow>
</template>
