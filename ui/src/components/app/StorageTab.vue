<script setup lang="ts">
import { useNodesStore } from '@/stores/nodes'
import { useStorageStore } from '@/stores/storage'
import StorageVolumeBar from '@/components/common/StorageVolumeBar.vue'

const storage = useStorageStore()
const nodes = useNodesStore()
</script>

<template>
  <div class="h-full overflow-y-auto p-3 grid gap-x-6 gap-y-3 grid-cols-[repeat(auto-fill,minmax(18rem,1fr))] content-start">
    <div v-for="[nodeId, vols] in storage.volumes" :key="nodeId" class="space-y-2">
      <h3 class="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
        {{ nodes.labelOf(nodeId) }}
      </h3>
      <p v-if="vols === null" class="text-xs text-destructive">Couldn't read storage.</p>
      <StorageVolumeBar v-for="v in vols ?? []" :key="v.mount_point" :volume="v" />
    </div>
  </div>
</template>
