<script setup lang="ts">
import { computed, ref } from 'vue'
import { useRafFn } from '@vueuse/core'
import { formatClock } from '@/lib/format'
import { useReplayStore, type ReplayBuffer } from '@/stores/replay'

/** How far the moment shown is behind live, run on every frame (by itself, so the page doesn't re-render). */
const props = defineProps<{ buffer: ReplayBuffer | null; showingUs: number | null }>()

const replay = useReplayStore()
const tick = ref(performance.now())
useRafFn(() => (tick.value = performance.now()))
const behind = computed(() =>
  props.buffer && props.showingUs !== null
    ? Math.max(0, replay.nowUs(props.buffer, tick.value) - props.showingUs) / 1000
    : null,
)
</script>

<template>
  <span>{{ behind === null ? '–' : `−${formatClock(behind)}` }}</span>
</template>
