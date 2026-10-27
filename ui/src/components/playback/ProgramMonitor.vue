<script setup lang="ts">
import { computed, ref, toRef } from 'vue'
import { Eye } from '@lucide/vue'
import { audioLevels, type Source } from '@/stores/sources'
import { fpsLabel } from '@/lib/sourceFormat'
import { plural } from '@/lib/format'
import type { ChannelStatusDto } from '@/types/generated/ChannelStatusDto'
import AudioMeter from '@/components/AudioMeter.vue'
import StatusDot from '@/components/common/StatusDot.vue'
import { useThumbnail } from '@/composables/useThumbnail'
import { TRANSPORT } from './transport'
import WhepViewer from './WhepViewer.vue'

/** A channel's program: its picture and meters, what's on it, and its outputs. */
const props = defineProps<{ source: Source; status?: ChannelStatusDto }>()

const thumb = useThumbnail(toRef(props, 'source'))
const channels = computed(() => audioLevels.get(props.source.key) ?? [])
const state = computed(() => TRANSPORT[props.status?.state ?? 'idle'])
/**
 * A WHEP output's endpoint, from its label (`WHEP /path`): through this instance, so an HTTPS
 * page never reaches a node itself.
 */
function whepEndpoint(label: string) {
  const node = encodeURIComponent(props.source.node_id)
  return label.startsWith('WHEP /') ? `${window.location.origin}/api/v1/nodes/${node}/whep${label.slice(5)}` : null
}
const watching = ref<string | null>(null)

const format = computed(() => {
  const c = props.source.capabilities
  return c ? `${c.max_width}×${c.max_height} · ${fpsLabel([c.fps_num, c.fps_den])} · ${c.audio_channels} ch` : null
})
</script>

<template>
  <div class="rounded-lg border border-border bg-card overflow-hidden">
    <div class="relative flex bg-black aspect-video">
      <div class="flex-1 relative overflow-hidden">
        <div
          v-if="source.error"
          class="absolute inset-0 flex flex-col items-center justify-center gap-1 px-4 text-center text-xs"
        >
          <span class="text-destructive font-medium">Program failed</span>
          <span class="text-zinc-400 line-clamp-3">{{ source.error }}</span>
        </div>
        <img
          v-else-if="thumb.src.value"
          :src="thumb.src.value"
          :alt="`${source.display_name} program`"
          class="w-full h-full object-contain"
          draggable="false"
          @error="thumb.onError"
        />
        <div v-else class="absolute inset-0 grid place-items-center text-zinc-500 text-xs">Not running</div>

        <span
          class="absolute top-2 left-2 rounded-sm px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide"
          :class="state.badge"
        >
          {{ state.label }}
        </span>
        <span
          v-if="status?.clip"
          class="absolute bottom-2 left-2 max-w-[70%] truncate rounded bg-black/70 px-1.5 py-0.5 text-[11px] text-white"
          :title="status.clip.name"
        >
          {{ status.clip.name }}
        </span>
        <span v-if="format" class="absolute bottom-2 right-2 rounded bg-black/70 px-1.5 py-0.5 text-[10px] num text-white">
          {{ format }}
        </span>
      </div>
      <div v-if="channels.length" class="w-8 py-1 shrink-0">
        <AudioMeter :channels="channels" />
      </div>
    </div>

    <div class="flex flex-wrap items-center gap-x-4 gap-y-1 px-3 py-2 text-xs">
      <span class="font-medium text-sm">{{ source.display_name }}</span>
      <span v-if="!status?.outputs.length" class="text-muted-foreground">No outputs: record it from Record, or add one in Channel settings.</span>
      <span
        v-for="o in status?.outputs ?? []"
        :key="o.label"
        class="flex items-center gap-1.5"
        :class="o.error ? 'text-destructive' : 'text-muted-foreground'"
        :title="o.error ?? 'Sending'"
      >
        <StatusDot :status="o.error ? 'error' : 'ok'" />
        {{ o.label }}<template v-if="o.error">: {{ o.error }}</template
        ><template v-else-if="o.receivers !== null"> · {{ plural(o.receivers, 'receiver') }}</template
        ><template v-else-if="o.detail"> · {{ o.detail }}</template>
        <button
          v-if="!o.error && whepEndpoint(o.label)"
          type="button"
          class="flex items-center gap-1 text-primary hover:underline"
          @click="watching = whepEndpoint(o.label)"
        >
          <Eye class="size-3.5" /> Watch
        </button>
      </span>
    </div>
  </div>
  <WhepViewer v-if="watching" :title="source.display_name" :endpoint="watching" @close="watching = null" />
</template>
