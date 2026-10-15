<script setup lang="ts">
import { computed, ref } from 'vue'
import type { ReplayBuffer } from '@/stores/replay'

/**
 * One angle's buffer on a strip running up to now (the right edge): what's
 * recorded, the second or so that can't be replayed yet, gaps, the in and
 * out marks and the playhead. Clicking or dragging picks a moment (node
 * clock, µs); it's emitted on release.
 */
const props = defineProps<{
  buffer: ReplayBuffer
  /** The node clock now, µs. */
  nowUs: number
  /** How far back the strip reaches, seconds; `null` for the whole buffer. */
  windowSecs: number | null
  playheadUs: number | null
  inUs: number | null
  outUs: number | null
}>()
const emit = defineEmits<{ pick: [us: number] }>()

const viewEnd = computed(() => props.nowUs)
const viewStart = computed(() => {
  const start = props.buffer.start_us ?? props.nowUs - 10_000_000
  return props.windowSecs === null ? Math.min(start, props.nowUs - 10_000_000) : props.nowUs - props.windowSecs * 1_000_000
})
const span = computed(() => Math.max(1, viewEnd.value - viewStart.value))
const pct = (us: number) => `${Math.min(100, Math.max(0, ((us - viewStart.value) / span.value) * 100))}%`
const width = (a: number, b: number) => `calc(${pct(b)} - ${pct(a)})`

const recorded = computed(() =>
  props.buffer.start_us !== null && props.buffer.end_us !== null
    ? { start: props.buffer.start_us, end: props.buffer.end_us }
    : null,
)

/** Tick marks every 5 s / 10 s / 1 min…, labelled as time before now. */
const ticks = computed(() => {
  const secs = span.value / 1_000_000
  const step = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 1800, 3600].find((s) => secs / s <= 8) ?? 3600
  const out: { us: number; label: string }[] = []
  for (let back = step; back * 1_000_000 < span.value; back += step) {
    const label = back >= 60 ? `−${Math.floor(back / 60)}:${String(back % 60).padStart(2, '0')}` : `−${back}s`
    out.push({ us: viewEnd.value - back * 1_000_000, label })
  }
  return out
})

const bar = ref<HTMLElement | null>(null)
const dragging = ref<number | null>(null)
function at(e: PointerEvent) {
  const r = bar.value!.getBoundingClientRect()
  const x = Math.min(Math.max(e.clientX - r.left, 0), r.width)
  return Math.round(viewStart.value + (x / r.width) * span.value)
}
function down(e: PointerEvent) {
  if (!recorded.value) return
  bar.value!.setPointerCapture(e.pointerId)
  dragging.value = at(e)
}
function move(e: PointerEvent) {
  if (dragging.value !== null) dragging.value = at(e)
}
function up() {
  if (dragging.value === null || !recorded.value) return
  const us = Math.min(Math.max(dragging.value, recorded.value.start), recorded.value.end)
  dragging.value = null
  emit('pick', us)
}
const head = computed(() => dragging.value ?? props.playheadUs)
</script>

<template>
  <div class="flex flex-col gap-1">
    <div
      ref="bar"
      class="relative h-10 rounded bg-muted touch-none overflow-hidden"
      :class="recorded ? 'cursor-pointer' : 'opacity-50'"
      role="slider"
      aria-label="Moment to replay"
      :aria-valuemin="recorded?.start ?? 0"
      :aria-valuemax="recorded?.end ?? 0"
      :aria-valuenow="head ?? undefined"
      @pointerdown="down"
      @pointermove="move"
      @pointerup="up"
      @pointercancel="dragging = null"
    >
      <!-- Recorded, and not replayable yet (the segment being written). -->
      <div
        v-if="recorded"
        class="absolute inset-y-0 bg-primary/25"
        :style="{ left: pct(recorded.start), width: width(recorded.start, recorded.end) }"
      />
      <div
        v-if="recorded"
        class="absolute inset-y-0 bg-[repeating-linear-gradient(135deg,transparent_0_4px,var(--color-border)_4px_6px)]"
        :style="{ left: pct(recorded.end), width: width(recorded.end, viewEnd) }"
        title="Not replayable yet"
      />
      <!-- Gaps: the source restarted. -->
      <div
        v-for="b in buffer.breaks_us"
        :key="b"
        class="absolute inset-y-0 w-px bg-destructive"
        :style="{ left: pct(b) }"
        title="Gap: the source restarted"
      />
      <!-- Ticks -->
      <template v-for="t in ticks" :key="t.label">
        <div class="absolute bottom-0 h-1.5 w-px bg-foreground/30" :style="{ left: pct(t.us) }" />
        <span class="absolute bottom-1.5 -translate-x-1/2 text-[9px] num text-muted-foreground pointer-events-none" :style="{ left: pct(t.us) }">
          {{ t.label }}
        </span>
      </template>
      <!-- In → out -->
      <div
        v-if="inUs !== null && outUs !== null && outUs > inUs"
        class="absolute top-0 h-2 bg-warning/60"
        :style="{ left: pct(inUs), width: width(inUs, outUs) }"
      />
      <div v-if="inUs !== null" class="absolute inset-y-0 w-0.5 bg-warning" :style="{ left: pct(inUs) }" title="In" />
      <div v-if="outUs !== null" class="absolute inset-y-0 w-0.5 bg-warning" :style="{ left: pct(outUs) }" title="Out" />
      <!-- Playhead -->
      <div v-if="head !== null" class="absolute inset-y-0 w-0.5 -ml-px bg-foreground" :style="{ left: pct(head) }" />
      <span class="absolute right-1.5 top-0.5 text-[9px] font-semibold uppercase tracking-wide text-muted-foreground pointer-events-none">
        Live
      </span>
    </div>
  </div>
</template>
