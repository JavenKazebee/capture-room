<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import type { ChannelLevelDto } from '@/types/generated/ChannelLevelDto'

const props = defineProps<{
  channels: ChannelLevelDto[]
}>()

// dB range
const DB_MIN = -60
const DB_MAX = 0

// Zones are fixed by position, like a hardware meter: the top of the scale is
// always red, whatever the level. One gradient is drawn for every layer and
// each layer reveals only its share of it.
const WARN_DB = -18
const HOT_DB = -6

// Ballistics: bars rise to a new level at once and fall at FALL_DB_PER_SEC,
// animated per frame so they glide between level messages instead of jumping.
const FALL_DB_PER_SEC = 15

// Peak hold: the held level stays put for PEAK_HOLD_MS, then falls.
const PEAK_HOLD_MS = 2000
const PEAK_FALL_DB_PER_SEC = 20

// A peak at or above CLIP_DB latches the channel's clip light until clicked.
const CLIP_DB = -0.5

// A channel whose peak stays at the bottom of the scale this long greys out —
// a dead mic or a missing embed.
const SILENT_MS = 3000

function dbToPercent(db: number): number {
  return Math.max(0, Math.min(100, ((db - DB_MIN) / (DB_MAX - DB_MIN)) * 100))
}

const ZONES = `linear-gradient(to top,
  var(--meter-ok) 0 ${dbToPercent(WARN_DB)}%,
  var(--meter-warn) ${dbToPercent(WARN_DB)}% ${dbToPercent(HOT_DB)}%,
  var(--meter-clip) ${dbToPercent(HOT_DB)}%)`

/** Silence arrives as -inf, which JSON carries as null. */
function finite(db: number | null): number {
  return db != null && Number.isFinite(db) ? db : -Infinity
}

// Per-channel state. Displayed levels change every frame, so they're written
// straight to the DOM; only the clip and silence flags, which change rarely,
// go through Vue. The layers' clip-paths start as static attributes in the
// template so a re-render never resets what the animation wrote.
type Bar = { peak: number; rms: number; holdTop: number; holdAt: number; loudAt: number }
let bars: Bar[] = []
const clipped = ref<boolean[]>([])
const silent = ref<boolean[]>([])

const peakEls: HTMLElement[] = []
const rmsEls: HTMLElement[] = []
const holdEls: HTMLElement[] = []

function heldDb(bar: Bar, now: number): number {
  const fallingMs = now - bar.holdAt - PEAK_HOLD_MS
  return fallingMs > 0 ? bar.holdTop - (PEAK_FALL_DB_PER_SEC * fallingMs) / 1000 : bar.holdTop
}

/** Reveal the bottom `db` of the scale; `band` reveals only a sliver at `db`. */
function reveal(el: HTMLElement | undefined, db: number, band = false) {
  if (!el) return
  const top = 100 - dbToPercent(db)
  el.style.clipPath = band ? `inset(${top}% 0 calc(${100 - top}% - 1px) 0)` : `inset(${top}% 0 0 0)`
}

watch(
  () => props.channels,
  (channels) => {
    const now = performance.now()
    if (channels.length !== bars.length) {
      bars = channels.map(() => ({ peak: DB_MIN, rms: DB_MIN, holdTop: -Infinity, holdAt: now, loudAt: now }))
      clipped.value = channels.map(() => false)
      silent.value = channels.map(() => false)
    }
    channels.forEach((ch, i) => {
      const bar = bars[i]!
      const peak = finite(ch.peak_db)
      if (peak >= heldDb(bar, now)) {
        bar.holdTop = peak
        bar.holdAt = now
      }
      if (peak > DB_MIN) bar.loudAt = now
      if (peak >= CLIP_DB && !clipped.value[i]) clipped.value[i] = true
      const isSilent = now - bar.loudAt >= SILENT_MS
      if (silent.value[i] !== isSilent) silent.value[i] = isSilent
    })
    start()
  },
)

let frame = 0
let lastAt = 0

function start() {
  if (frame) return
  lastAt = performance.now()
  frame = requestAnimationFrame(tick)
}

function tick(now: number) {
  const fall = (FALL_DB_PER_SEC * Math.max(0, now - lastAt)) / 1000
  lastAt = now
  let moving = false
  props.channels.forEach((ch, i) => {
    const bar = bars[i]
    if (!bar) return
    bar.peak = Math.max(finite(ch.peak_db), bar.peak - fall, DB_MIN)
    bar.rms = Math.max(finite(ch.rms_db), bar.rms - fall, DB_MIN)
    const held = heldDb(bar, now)
    reveal(peakEls[i], bar.peak)
    reveal(rmsEls[i], bar.rms)
    reveal(holdEls[i], held, true)
    moving ||= bar.peak > DB_MIN || bar.rms > DB_MIN || held > DB_MIN
  })
  // Settled at the floor: sleep until the next level message.
  frame = moving ? requestAnimationFrame(tick) : 0
}

function resetClips() {
  clipped.value = clipped.value.map(() => false)
}

onMounted(start)
onBeforeUnmount(() => cancelAnimationFrame(frame))
</script>

<template>
  <div class="flex gap-0.5 h-full px-1" :style="{ '--zones': ZONES }">
    <div v-for="(_, i) in channels" :key="i" class="flex flex-1 flex-col gap-0.5" style="min-width: 6px">
      <!-- Clip light: latches on an over, click to reset every channel -->
      <button
        type="button"
        class="h-1.5 shrink-0 rounded-sm"
        :class="clipped[i] ? 'bg-meter-clip cursor-pointer' : 'bg-muted/50 cursor-default'"
        :title="clipped[i] ? 'Clipped — click to reset' : undefined"
        :aria-label="clipped[i] ? `Channel ${i + 1} clipped, reset` : `Channel ${i + 1} clip light`"
        @click.stop="resetClips"
      />

      <div class="relative flex-1 overflow-hidden rounded-sm" :class="silent[i] ? 'bg-muted/40' : ''">
        <!-- Track: the zones, faint, so the scale reads even when idle -->
        <div v-if="!silent[i]" class="absolute inset-0 opacity-15" style="background: var(--zones)" />

        <!-- Peak: the full extent, dimmer -->
        <div
          :ref="(el) => (peakEls[i] = el as HTMLElement)"
          class="absolute inset-0 opacity-45"
          style="background: var(--zones); clip-path: inset(100% 0 0 0)"
        />

        <!-- RMS: the body, solid -->
        <div
          :ref="(el) => (rmsEls[i] = el as HTMLElement)"
          class="absolute inset-0"
          style="background: var(--zones); clip-path: inset(100% 0 0 0)"
        />

        <!-- Peak hold line -->
        <div
          :ref="(el) => (holdEls[i] = el as HTMLElement)"
          class="absolute inset-0"
          style="background: var(--zones); clip-path: inset(100% 0 0 0)"
        />
      </div>
    </div>
  </div>
</template>
