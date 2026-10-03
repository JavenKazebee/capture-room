<script setup lang="ts">
import { formatBytes } from '@/lib/format'
import { useNodesStore } from '@/stores/nodes'
import { useStorageStore } from '@/stores/storage'
import type { StorageVolumeDto } from '@/types/generated/StorageVolumeDto'

const storage = useStorageStore()
const nodes = useNodesStore()

function usedPct(v: StorageVolumeDto) {
  return v.total_bytes ? Math.round(((v.total_bytes - v.available_bytes) / v.total_bytes) * 100) : 0
}

function barClass(v: StorageVolumeDto) {
  const p = usedPct(v)
  return p > 90 ? 'bg-destructive' : p > 75 ? 'bg-warning' : 'bg-primary'
}
</script>

<template>
  <div class="h-full overflow-y-auto p-3 grid gap-x-6 gap-y-3 grid-cols-[repeat(auto-fill,minmax(18rem,1fr))] content-start">
    <div v-for="[nodeId, vols] in storage.volumes" :key="nodeId" class="space-y-2">
      <h3 class="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
        {{ nodes.labelOf(nodeId) }}
      </h3>
      <p v-if="vols === null" class="text-xs text-destructive">Couldn't read storage.</p>
      <div v-for="v in vols ?? []" :key="v.mount_point" class="text-xs">
        <div class="flex justify-between gap-2 mb-1">
          <span class="num truncate" :title="[v.mount_point, ...v.other_mounts].join('\n')">
            {{ v.mount_point }}
            <span class="text-muted-foreground font-sans">{{ v.removable ? '· removable' : '' }}</span>
          </span>
          <span class="num text-muted-foreground shrink-0">
            {{ formatBytes(v.available_bytes) }} free / {{ formatBytes(v.total_bytes) }}
          </span>
        </div>
        <div class="h-1.5 rounded-full bg-muted overflow-hidden">
          <div class="h-full rounded-full" :class="barClass(v)" :style="{ width: `${usedPct(v)}%` }" />
        </div>
      </div>
    </div>
  </div>
</template>
