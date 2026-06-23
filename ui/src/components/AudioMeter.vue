<script setup lang="ts">
import { ref, watch } from 'vue'

interface ChannelLevel {
  peak_db: number
  rms_db: number
}

const props = defineProps<{
  channels: ChannelLevel[]
}>()

// dB range
const DB_MIN = -60
const DB_MAX = 0

// Peak hold: value and when it was last updated (ms)
const peaks = ref<{ db: number; at: number }[]>([])
const PEAK_HOLD_MS = 2000
const PEAK_FALL_DB_PER_SEC = 20

let lastTick = Date.now()
let rafId: number | null = null

function dbToPercent(db: number): number {
  return Math.max(0, Math.min(100, ((db - DB_MIN) / (DB_MAX - DB_MIN)) * 100))
}

function levelColor(db: number): string {
  if (db >= -6) return '#ef4444'   // red
  if (db >= -18) return '#eab308'  // yellow
  return '#22c55e'                 // green
}

function tick() {
  const now = Date.now()
  const dt = (now - lastTick) / 1000
  lastTick = now

  // Fall peaks
  peaks.value = peaks.value.map((p) => {
    if (now - p.at > PEAK_HOLD_MS) {
      return { db: p.db - PEAK_FALL_DB_PER_SEC * dt, at: p.at }
    }
    return p
  })

  rafId = requestAnimationFrame(tick)
}

// Sync peaks when channels update
watch(
  () => props.channels,
  (channels) => {
    const now = Date.now()
    peaks.value = channels.map((ch, i) => {
      const prev = peaks.value[i]
      if (!prev || ch.peak_db >= prev.db) {
        return { db: ch.peak_db, at: now }
      }
      return prev
    })
  },
  { immediate: true },
)

// Start/stop RAF
import { onMounted, onUnmounted } from 'vue'
onMounted(() => { rafId = requestAnimationFrame(tick) })
onUnmounted(() => { if (rafId !== null) cancelAnimationFrame(rafId) })
</script>

<template>
  <div class="flex gap-0.5 h-full items-end px-1">
    <div
      v-for="(ch, i) in channels"
      :key="i"
      class="relative flex-1 h-full"
      style="min-width: 6px"
    >
      <!-- Track background -->
      <div class="absolute inset-0 rounded-sm bg-muted/50" />

      <!-- RMS fill -->
      <div
        class="absolute bottom-0 left-0 right-0 rounded-sm transition-none"
        :style="{
          height: dbToPercent(ch.rms_db) + '%',
          background: levelColor(ch.rms_db),
          opacity: '0.85',
        }"
      />

      <!-- Peak fill (slightly brighter) -->
      <div
        class="absolute bottom-0 left-0 right-0 rounded-sm"
        :style="{
          height: dbToPercent(ch.peak_db) + '%',
          background: levelColor(ch.peak_db),
          opacity: '0.4',
        }"
      />

      <!-- Peak hold line -->
      <div
        v-if="peaks[i] && peaks[i].db > DB_MIN"
        class="absolute left-0 right-0 h-px"
        :style="{
          bottom: dbToPercent(peaks[i].db) + '%',
          background: levelColor(peaks[i].db),
        }"
      />
    </div>
  </div>
</template>
