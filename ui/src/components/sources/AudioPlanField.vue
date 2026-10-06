<script setup lang="ts">
import { computed } from 'vue'
import type { AudioPlan } from '@/types/generated/AudioPlan'
import type { DeviceDto } from '@/types/generated/DeviceDto'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'

/**
 * Where a live source's audio comes from: its own (`sourceLabel`, hidden
 * when the source has none, like a camera), an audio device on the node
 * with picked channels, or silence.
 */
const props = defineProps<{
  /** The node's devices; only audio inputs are offered. */
  devices: DeviceDto[]
  /** "Stream", "Publisher"; omit for sources without audio of their own. */
  sourceLabel?: string
}>()

const plan = defineModel<AudioPlan>({ required: true })

const CHANNEL_OPTIONS = [
  { value: 1, label: 'Mono' },
  { value: 2, label: 'Stereo' },
  { value: 6, label: '5.1' },
  { value: 8, label: '7.1' },
]

const inputs = computed(() => props.devices.filter((d) => d.kind === 'audio'))
const inputOptions = computed(() =>
  inputs.value.map((d) => ({ value: d.key, label: `${d.name} (${d.channels ?? 2} ch)` })),
)
const current = computed(() => {
  const p = plan.value
  return p.from === 'device' ? inputs.value.find((d) => d.key === p.device_key) : undefined
})

type From = AudioPlan['from']
const from = computed({
  get: () => plan.value.from,
  set: (v: From | undefined) => {
    if (!v || v === plan.value.from) return
    if (v === 'device') {
      const first = inputs.value[0]
      plan.value = {
        from: 'device',
        device_key: first?.key ?? '',
        channels: (first?.channels ?? 2) >= 2 ? [1, 2] : [1],
      }
    } else {
      const channels = plan.value.from === 'device' ? Math.max(plan.value.channels.length, 1) : plan.value.channels
      plan.value = { from: v, channels }
    }
  },
})

const deviceKey = computed({
  get: () => (plan.value.from === 'device' ? plan.value.device_key : ''),
  set: (key: string) => {
    const device = inputs.value.find((d) => d.key === key)
    plan.value = { from: 'device', device_key: key, channels: (device?.channels ?? 2) >= 2 ? [1, 2] : [1] }
  },
})

const channelCount = computed({
  get: () => (plan.value.from === 'device' ? plan.value.channels.length : plan.value.channels),
  set: (n: number) => {
    if (plan.value.from !== 'device') plan.value = { ...plan.value, channels: n }
  },
})

/** Click order is output order: the first picked is left (or channel 1). */
function toggle(ch: number) {
  if (plan.value.from !== 'device') return
  const picked = plan.value.channels
  plan.value = {
    ...plan.value,
    channels: picked.includes(ch) ? picked.filter((c) => c !== ch) : [...picked, ch],
  }
}

const pickedLabel = computed(() => {
  if (plan.value.from !== 'device') return ''
  const c = plan.value.channels
  if (c.length === 0) return 'Pick at least one channel'
  if (c.length === 1) return `Mono from input ${c[0]}`
  if (c.length === 2) return `L = input ${c[0]}, R = input ${c[1]}`
  return c.map((ch, i) => `${i + 1} ← ${ch}`).join(', ')
})
</script>

<template>
  <div class="flex flex-col gap-2">
    <FormField label="Audio">
      <ToggleGroup v-model="from" type="single" variant="segmented" class="w-full">
        <ToggleGroupItem v-if="sourceLabel" value="source" class="flex-1 h-8 text-xs">{{ sourceLabel }}</ToggleGroupItem>
        <ToggleGroupItem value="device" class="flex-1 h-8 text-xs" :disabled="inputs.length === 0">
          Audio input
        </ToggleGroupItem>
        <ToggleGroupItem value="silence" class="flex-1 h-8 text-xs">Silence</ToggleGroupItem>
      </ToggleGroup>
    </FormField>

    <template v-if="plan.from === 'device'">
      <OptionSelect v-model="deviceKey" :options="inputOptions" />
      <div v-if="current" class="flex flex-col gap-1">
        <div class="flex flex-wrap gap-1">
          <button
            v-for="ch in current.channels ?? 2"
            :key="ch"
            type="button"
            class="h-7 min-w-7 px-1.5 rounded border text-xs num transition-colors"
            :class="
              plan.channels.includes(ch)
                ? 'bg-primary border-primary text-primary-foreground'
                : 'border-border text-muted-foreground hover:text-foreground hover:bg-accent'
            "
            :aria-pressed="plan.channels.includes(ch)"
            @click="toggle(ch)"
          >
            {{ ch }}
          </button>
        </div>
        <p class="text-xs" :class="plan.channels.length ? 'text-muted-foreground' : 'text-destructive'">
          {{ pickedLabel }}
        </p>
      </div>
    </template>
    <template v-else>
      <FormField :label="plan.from === 'source' ? 'Channels (mixed to)' : 'Channels'">
        <OptionSelect v-model="channelCount" :options="CHANNEL_OPTIONS" />
      </FormField>
      <p v-if="plan.from === 'source'" class="text-xs text-muted-foreground">
        Plays silence while the source has no audio.
      </p>
    </template>
    <p v-if="inputs.length === 0" class="text-xs text-muted-foreground">No audio inputs on this node.</p>
  </div>
</template>
