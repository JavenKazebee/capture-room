<script setup lang="ts">
import { computed } from 'vue'
import { formatBytes, formatRate, formatTimeLeft } from '@/lib/format'
import { LOW_TIME_SECS } from '@/stores/capacity'
import type { StorageVolumeDto } from '@/types/generated/StorageVolumeDto'

/**
 * One volume: mount point, free space and a fill bar (warning past 75%,
 * destructive past 90%). While recordings write to it, also the rate and the
 * recording time that leaves (warning under an hour).
 */
const props = defineProps<{ volume: StorageVolumeDto }>()

const usedPct = computed(() => {
  const v = props.volume
  return v.total_bytes ? Math.round(((v.total_bytes - v.available_bytes) / v.total_bytes) * 100) : 0
})

const barClass = computed(() =>
  usedPct.value > 90 ? 'bg-destructive' : usedPct.value > 75 ? 'bg-warning' : 'bg-primary',
)

const low = computed(() => props.volume.seconds_left != null && props.volume.seconds_left < LOW_TIME_SECS)
</script>

<template>
  <div class="text-xs">
    <div class="flex justify-between gap-2 mb-1">
      <span class="num truncate" :title="[volume.mount_point, ...volume.other_mounts].join('\n')">
        {{ volume.mount_point }}
        <span class="text-muted-foreground font-sans">{{ volume.removable ? '· removable' : '' }}</span>
      </span>
      <span class="num text-muted-foreground shrink-0">
        {{ formatBytes(volume.available_bytes) }} free / {{ formatBytes(volume.total_bytes) }}
      </span>
    </div>
    <div class="h-1.5 rounded-full bg-muted overflow-hidden">
      <div class="h-full rounded-full" :class="barClass" :style="{ width: `${usedPct}%` }" />
    </div>
    <p v-if="volume.seconds_left != null" class="mt-1 flex justify-between gap-2 text-muted-foreground">
      <span>Recording <span class="num">{{ formatRate(volume.write_bytes_per_sec) }}</span></span>
      <span :class="low && 'text-warning font-medium'">
        <span class="num">{{ formatTimeLeft(volume.seconds_left) }}</span> left at this rate
      </span>
    </p>
  </div>
</template>
