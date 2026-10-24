<script setup lang="ts">
import { computed, ref } from 'vue'
import { useRafFn } from '@vueuse/core'
import { formatClock } from '@/lib/format'
import { useReplayStore, type ReplayBuffer } from '@/stores/replay'

/**
 * How far the moment shown is behind live, run on every frame (by itself, so the page doesn't
 * re-render), and while playing which way that's going.
 */
const props = defineProps<{
  buffer: ReplayBuffer | null
  showingUs: number | null
  /** The replay's speed while it plays; `null` otherwise. */
  playingRate: number | null
}>()

/** Closer than this, a replay is as near live as the buffer reaches: about a segment. */
const EDGE_MS = 2500

const replay = useReplayStore()
const tick = ref(performance.now())
useRafFn(() => (tick.value = performance.now()))
const behind = computed(() =>
  props.buffer && props.showingUs !== null
    ? Math.max(0, replay.nowUs(props.buffer, tick.value) - props.showingUs) / 1000
    : null,
)

/** Live runs at 1×: slower falls behind, faster catches up. */
const trend = computed(() => {
  const r = props.playingRate
  if (r === null || behind.value === null) return null
  if (r > 0 && behind.value < EDGE_MS) return { text: 'at the live edge', tone: 'text-success' }
  if (r > 1) return { text: '▲ catching up', tone: 'text-success' }
  if (r === 1) return { text: 'keeping pace', tone: 'text-muted-foreground' }
  return { text: '▼ falling behind', tone: 'text-warning' }
})
</script>

<template>
  <span class="flex flex-col items-end">
    <span>{{ behind === null ? '–' : `−${formatClock(behind)}` }}</span>
    <span v-if="trend" class="text-[10px] font-sans leading-none" :class="trend.tone">{{ trend.text }}</span>
  </span>
</template>
