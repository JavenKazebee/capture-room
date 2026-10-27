<script setup lang="ts">
import { computed } from 'vue'
import { TriangleAlert } from '@lucide/vue'
import type { OutputConfig } from '@/types/generated/OutputConfig'
import { RTSP_PORT, mountPath, whepUrl } from '@/lib/streamUrl'
import { Input } from '@/components/ui/input'
import CopyButton from '@/components/common/CopyButton.vue'
import BitrateField from './BitrateField.vue'
import OutputRow from './OutputRow.vue'

/** The two outputs the node serves at a path: RTSP and WHEP. */
type ServedOutput = Extract<OutputConfig, { type: 'rtsp' | 'whep' }>

const props = defineProps<{
  output: ServedOutput
  index: number
  /** The channel's name: the path unless one is set. */
  name: string
  /** Where receivers reach the node, for RTSP addresses. */
  host: string
  /** The node's API origin as receivers reach it, for WHEP addresses. */
  origin: string
}>()
const emit = defineEmits<{ update: [patch: Partial<ServedOutput>]; remove: [] }>()

const label = computed(() => (props.output.type === 'rtsp' ? 'RTSP' : 'WHEP'))
const viewUrl = computed(() =>
  props.output.type === 'rtsp'
    ? `rtsp://${props.host}:${RTSP_PORT}${mountPath(props.output.path, props.name)}`
    : whepUrl(props.origin, props.output.path, props.name),
)
</script>

<template>
  <OutputRow :label="label" :index="index" @remove="emit('remove')">
    <template #control>
      <Input
        :model-value="output.path ?? ''"
        class="h-7 flex-1 font-mono text-[0.6875rem]"
        :placeholder="mountPath(null, name)"
        :aria-label="`${label} path for output ${index + 1}`"
        @update:model-value="(v) => emit('update', { path: String(v) || null })"
      />
    </template>

    <div class="grid grid-cols-3 gap-2">
      <BitrateField :model-value="output.bitrate_kbps" @update:model-value="(v) => emit('update', { bitrate_kbps: v })" />
    </div>
    <div class="flex flex-wrap items-center gap-1 text-xs">
      <span class="text-muted-foreground">Viewers open</span>
      <span class="font-mono break-all">{{ viewUrl }}</span>
      <CopyButton :value="viewUrl" />
    </div>
    <p v-if="output.type === 'rtsp'" class="text-xs text-muted-foreground">
      Encoded only while someone is watching; every viewer shares one encode.
    </p>
    <template v-else>
      <p class="text-xs text-muted-foreground">
        In a WHEP player, or Watch on the channel in Playback. Each viewer gets their own encode, sized to their
        connection; audio is mixed to stereo.
      </p>
      <p class="flex items-start gap-1.5 text-xs text-muted-foreground">
        <TriangleAlert class="size-3.5 shrink-0 mt-px text-warning" />
        No authentication yet: anyone who can reach the node can watch.
      </p>
    </template>
  </OutputRow>
</template>
