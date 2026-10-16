<script setup lang="ts">
import { computed } from 'vue'
import { RefreshCw } from '@lucide/vue'
import type { DecklinkSourceConfig } from '@/types/generated/DecklinkSourceConfig'
import type { DecklinkStatusDto } from '@/types/generated/DecklinkStatusDto'
import type { DecklinkConnection } from '@/types/generated/DecklinkConnection'
import { findSubDevice, subDeviceOptions } from '@/lib/decklink'
import { Button } from '@/components/ui/button'
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'
import DecklinkProfileField from './DecklinkProfileField.vue'

/** A Decklink sub-device's input: which connector, its mode and its embedded audio. */
const props = defineProps<{
  nodeId: string
  /** The source being edited, so its own use of the sub-device isn't flagged. */
  sourceId: string | null
  status: DecklinkStatusDto
  loading: boolean
  error: string | null
}>()
const emit = defineEmits<{ refresh: [] }>()

const cfg = defineModel<DecklinkSourceConfig>({ required: true })

const picked = computed(() => findSubDevice(props.status, cfg.value.device))

const deviceOptions = computed(() => {
  const opts = subDeviceOptions(props.status, false, props.sourceId)
  // A saved sub-device that's missing stays visible, so the form doesn't
  // silently pick another.
  if (cfg.value.device && !picked.value) {
    opts.unshift({ value: cfg.value.device, label: `${cfg.value.device_name || cfg.value.device} (not connected)` })
  }
  return opts
})

const device = computed({
  get: () => cfg.value.device,
  set: (key: string) => (cfg.value = { ...cfg.value, device: key, mode: null }),
})

const CONNECTIONS: { value: DecklinkConnection; label: string }[] = [
  { value: 'auto', label: 'Automatic' },
  { value: 'sdi', label: 'SDI' },
  { value: 'hdmi', label: 'HDMI' },
  { value: 'optical-sdi', label: 'Optical SDI' },
]

const AUTO = 'auto'
const modeOptions = computed(() => [
  { value: AUTO, label: 'Detect from the signal' },
  ...(picked.value?.device.modes ?? []).map((m) => ({ value: m.mode, label: m.label })),
])
const mode = computed({
  get: () => cfg.value.mode ?? AUTO,
  set: (v: string) => (cfg.value = { ...cfg.value, mode: v === AUTO ? null : v }),
})

const CHANNELS = [
  { value: 2, label: '2 channels' },
  { value: 8, label: '8 channels' },
  { value: 16, label: '16 channels' },
]
</script>

<template>
  <div class="col-span-2 flex flex-col gap-3">
    <p v-if="status.unavailable" class="text-xs text-destructive">{{ status.unavailable }}</p>
    <FormField label="Input">
      <div class="flex gap-1.5">
        <OptionSelect v-model="device" :options="deviceOptions" :disabled="!deviceOptions.length" />
        <Button
          type="button"
          variant="outline"
          size="icon"
          class="shrink-0"
          title="Refresh cards"
          :disabled="loading"
          @click="emit('refresh')"
        >
          <RefreshCw class="size-3.5" :class="loading && 'animate-spin'" />
        </Button>
      </div>
    </FormField>
    <p v-if="error" class="-mt-1.5 text-xs text-destructive">{{ error }}</p>
    <p v-else-if="!loading && !deviceOptions.length && !status.unavailable" class="-mt-1.5 text-xs text-muted-foreground">
      No Decklink inputs on this node. Check the card shows in Blackmagic Desktop Video Setup, then refresh.
    </p>

    <template v-if="picked">
      <div class="grid grid-cols-2 gap-3">
        <FormField label="Connection">
          <OptionSelect v-model="cfg.connection" :options="CONNECTIONS" />
        </FormField>
        <FormField label="Embedded audio">
          <OptionSelect v-model="cfg.channels" :options="CHANNELS" />
        </FormField>
      </div>
      <FormField label="Video mode">
        <OptionSelect v-model="mode" :options="modeOptions" />
      </FormField>
      <p class="-mt-1.5 text-xs text-muted-foreground">
        Detected modes follow the signal. The feed keeps its first format, so a camera switched to another mode is
        scaled into it. Without a signal the feed is black and silent, and recordings keep going.
      </p>
      <DecklinkProfileField :node-id="nodeId" :card="picked.card" @changed="emit('refresh')" />
    </template>
  </div>
</template>
