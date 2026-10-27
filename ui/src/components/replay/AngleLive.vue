<script setup lang="ts">
import { computed } from 'vue'
import { useThumbnail } from '@/composables/useThumbnail'
import { held } from './held'
import type { Angle } from './useReplayControl'

/** The selected angle, live, with what its buffer holds. */
const props = defineProps<{ angle: Angle }>()

const thumb = useThumbnail(computed(() => props.angle.source))
</script>

<template>
  <div class="rounded-lg border border-border overflow-hidden bg-card">
    <div class="relative aspect-video bg-black">
      <img
        v-if="thumb.src.value"
        :src="thumb.src.value"
        :alt="`${angle.source.display_name} live`"
        class="w-full h-full object-contain"
        draggable="false"
        @error="thumb.onError"
      />
      <span class="absolute top-1.5 left-1.5 rounded-sm bg-black/70 px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-zinc-300">
        Live
      </span>
    </div>
    <div class="px-2 py-1.5 text-xs flex flex-col gap-0.5">
      <span class="font-medium truncate">{{ angle.source.display_name }}</span>
      <span class="text-muted-foreground num">{{ held(angle.buffer) }} held · {{ angle.buffer.encoder ?? 'not recording' }}</span>
    </div>
  </div>
</template>
