<script setup lang="ts">
import { computed, ref } from 'vue'
import type { OutputConfig } from '@/types/generated/OutputConfig'
import { hostPort, srtListens, srtMode, withSrtMode } from '@/lib/streamUrl'
import { Input } from '@/components/ui/input'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import FormField from '@/components/FormField.vue'
import CopyButton from '@/components/common/CopyButton.vue'
import BitrateField from './BitrateField.vue'
import OutputRow from './OutputRow.vue'

type SrtOutput = Extract<OutputConfig, { type: 'srt' }>

const props = defineProps<{
  output: SrtOutput
  index: number
  /** Where receivers reach the node, for a listener's address. */
  host: string
}>()
const emit = defineEmits<{ update: [patch: Partial<SrtOutput>]; remove: [] }>()

/** The caller's host, kept while toggling to listener and back. */
const callerHost = ref('')

function setMode(mode: unknown) {
  const url = props.output.url
  if ((mode !== 'listener' && mode !== 'caller') || mode === srtMode(url)) return
  if (mode === 'listener') callerHost.value = hostPort(url).host
  emit('update', { url: withSrtMode(url, mode, callerHost.value) })
}

/** Where a receiver connects, for a listener. */
const receiveAt = computed(() => {
  const { port } = hostPort(props.output.url)
  return srtListens(props.output.url) && port ? `srt://${props.host}:${port}` : null
})
</script>

<template>
  <OutputRow label="SRT" :index="index" @remove="emit('remove')">
    <template #control>
      <ToggleGroup
        :model-value="srtMode(output.url)"
        type="single"
        variant="segmented"
        class="flex-1 min-w-0"
        @update:model-value="setMode"
      >
        <ToggleGroupItem value="listener" class="flex-1 h-7 text-xs">Listener</ToggleGroupItem>
        <ToggleGroupItem value="caller" class="flex-1 h-7 text-xs">Caller</ToggleGroupItem>
      </ToggleGroup>
    </template>

    <Input
      :model-value="output.url"
      class="h-7 font-mono text-[0.6875rem]"
      placeholder="srt://:9000"
      :aria-label="`SRT URL for output ${index + 1}`"
      @update:model-value="(v) => emit('update', { url: String(v) })"
    />
    <div class="grid grid-cols-3 gap-2">
      <BitrateField :model-value="output.bitrate_kbps" @update:model-value="(v) => emit('update', { bitrate_kbps: v })" />
      <FormField label="Latency (ms)">
        <Input
          :model-value="output.latency_ms"
          type="number"
          min="20"
          max="8000"
          class="h-7"
          @update:model-value="(v) => emit('update', { latency_ms: Number(v) })"
        />
      </FormField>
      <FormField label="Passphrase">
        <Input
          :model-value="output.passphrase ?? ''"
          type="password"
          autocomplete="off"
          placeholder="Optional"
          class="h-7"
          @update:model-value="(v) => emit('update', { passphrase: String(v) || null })"
        />
      </FormField>
    </div>
    <div v-if="receiveAt" class="flex flex-wrap items-center gap-1 text-xs">
      <span class="text-muted-foreground">Receivers connect to</span>
      <span class="font-mono">{{ receiveAt }}<template v-if="output.passphrase">?passphrase=…</template></span>
      <CopyButton :value="receiveAt" />
    </div>
    <p v-else class="text-xs text-muted-foreground">Sends to the receiver at this address, retrying until it answers.</p>
  </OutputRow>
</template>
