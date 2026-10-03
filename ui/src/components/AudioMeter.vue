<script setup lang="ts">
import { ref, watch } from 'vue'
import type { ChannelLevelDto } from '@/types/generated/ChannelLevelDto'

const props = defineProps<{
  channels: ChannelLevelDto[]
}>()

// dB range
const DB_MIN = -60
const DB_MAX = 0

// Peak hold: the held level stays put for PEAK_HOLD_MS, then falls. It is
// advanced whenever new levels arrive — those re-render the meter anyway, so
// there's no animation loop of its own. `top` and `at` are the level and time
// the hold was last raised; `db` is where it currently sits.
const PEAK_HOLD_MS = 2000
const PEAK_FALL_DB_PER_SEC = 20

type Peak = { top: number; at: number; db: number }
const peaks = ref<Peak[]>([])

function dbToPercent(db: number): number {
  return Math.max(0, Math.min(100, ((db - DB_MIN) / (DB_MAX - DB_MIN)) * 100))
}

function levelColor(db: number): string {
  if (db >= -6) return 'var(--meter-clip)'
  if (db >= -18) return 'var(--meter-warn)'
  return 'var(--meter-ok)'
}

function heldDb(peak: Peak, now: number): number {
  const fallingMs = now - peak.at - PEAK_HOLD_MS
  return fallingMs > 0 ? peak.top - (PEAK_FALL_DB_PER_SEC * fallingMs) / 1000 : peak.top
}

watch(
  () => props.channels,
  (channels) => {
    const now = performance.now()
    peaks.value = channels.map((ch, i) => {
      const prev = peaks.value[i]
      const held = prev ? heldDb(prev, now) : -Infinity
      return ch.peak_db >= held ? { top: ch.peak_db, at: now, db: ch.peak_db } : { ...prev!, db: held }
    })
  },
  { immediate: true },
)
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
