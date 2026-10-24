<script setup lang="ts">
import { computed, ref } from 'vue'
import { useRafFn } from '@vueuse/core'
import { useReplayStore, type ReplayBuffer } from '@/stores/replay'

/**
 * One angle's buffer on a strip ending at now. Click or drag picks a moment (node clock, µs), emitted
 * on release. While a replay plays the strip holds still, so the playhead visibly moves along the
 * footage (paging on near the right edge); then it eases back to live.
 */
const props = defineProps<{
  buffer: ReplayBuffer
  /** How far back the strip reaches, seconds; `null` for the whole buffer. */
  windowSecs: number | null
  playheadUs: number | null
  /** The playhead is another angle's replay, at that moment. */
  playheadOther: boolean
  /** A replay is playing: hold the strip still. */
  playing: boolean
  inUs: number | null
  outUs: number | null
}>()
const emit = defineEmits<{ pick: [us: number] }>()

/** The playhead stays this far (of the strip) from its right edge while held. */
const PAGE_AHEAD = 0.1
/** Easing back to live, ms. */
const SETTLE_MS = 150

// The strip runs every frame: its own clock, so only it re-renders.
const replay = useReplayStore()
const tick = ref(performance.now())
/** The node clock now, µs. */
const nowUs = computed(() => replay.nowUs(props.buffer, tick.value))

const viewStartAt = (end: number) => {
  const start = props.buffer.start_us ?? end - 10_000_000
  return props.windowSecs === null ? Math.min(start, end - 10_000_000) : end - props.windowSecs * 1_000_000
}
/** How far the strip's end is behind now, µs: 0 following live, more while held. */
const lag = ref(0)
useRafFn(({ delta }) => {
  // Held, the strip ends where it did last frame.
  const held = nowUs.value - lag.value
  tick.value = performance.now()
  const now = nowUs.value
  if (props.playing) {
    let end = Math.min(held, now)
    if (props.playheadUs !== null) {
      const ahead = (end - viewStartAt(end)) * PAGE_AHEAD
      end = Math.max(end, Math.min(now, props.playheadUs + ahead))
    }
    lag.value = now - end
  } else {
    const eased = lag.value * Math.exp(-delta / SETTLE_MS)
    lag.value = eased < 20_000 ? 0 : eased
  }
})
/** Where the strip ends: now, or held while a replay plays. */
const viewEnd = computed(() => nowUs.value - lag.value)
/** Following live, not held. */
const live = computed(() => lag.value === 0)
const viewStart = computed(() => viewStartAt(viewEnd.value))
const span = computed(() => Math.max(1, viewEnd.value - viewStart.value))
const pct = (us: number) => `${Math.min(100, Math.max(0, ((us - viewStart.value) / span.value) * 100))}%`
const width = (a: number, b: number) => `calc(${pct(b)} - ${pct(a)})`

const recorded = computed(() =>
  props.buffer.start_us !== null && props.buffer.end_us !== null
    ? { start: props.buffer.start_us, end: props.buffer.end_us }
    : null,
)

/** Tick marks every 5 s / 10 s / 1 min… back from the strip's end, labelled as time before now. */
const ticks = computed(() => {
  const secs = span.value / 1_000_000
  const step = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 1800, 3600].find((s) => secs / s <= 8) ?? 3600
  const out: { us: number; label: string }[] = []
  for (let back = step; back * 1_000_000 < span.value; back += step) {
    const us = viewEnd.value - back * 1_000_000
    const ago = Math.round((nowUs.value - us) / 1_000_000)
    const label = ago >= 60 ? `−${Math.floor(ago / 60)}:${String(ago % 60).padStart(2, '0')}` : `−${ago}s`
    out.push({ us, label })
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
      <template v-for="t in ticks" :key="t.us">
        <div class="absolute bottom-0 h-1.5 w-px bg-foreground/30" :style="{ left: pct(t.us) }" />
        <span class="absolute bottom-1.5 -translate-x-1/2 text-[9px] num text-muted-foreground pointer-events-none" :style="{ left: pct(t.us) }">
          {{ t.label }}
        </span>
      </template>
      <div
        v-if="inUs !== null && outUs !== null && outUs > inUs"
        class="absolute top-0 h-2 bg-warning/60"
        :style="{ left: pct(inUs), width: width(inUs, outUs) }"
      />
      <div v-if="inUs !== null" class="absolute inset-y-0 w-0.5 bg-warning" :style="{ left: pct(inUs) }" title="In" />
      <div v-if="outUs !== null" class="absolute inset-y-0 w-0.5 bg-warning" :style="{ left: pct(outUs) }" title="Out" />
      <div
        v-if="head !== null"
        class="absolute inset-y-0 -ml-px"
        :class="playheadOther && dragging === null ? 'border-l-2 border-dashed border-foreground/60' : 'w-0.5 bg-foreground'"
        :style="{ left: pct(head) }"
        :title="playheadOther && dragging === null ? 'The moment on air, on another angle' : undefined"
      />
      <!-- Held, live is further right. -->
      <span class="absolute right-1.5 top-0.5 text-[9px] font-semibold uppercase tracking-wide text-muted-foreground pointer-events-none">
        {{ live ? 'Live' : 'Live →' }}
      </span>
    </div>
  </div>
</template>
