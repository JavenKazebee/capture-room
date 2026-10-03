import { defineStore } from 'pinia'
import { computed, reactive } from 'vue'
import { nodeApi } from '@/composables/useApi'
import { useNodesStore } from '@/stores/nodes'
import type { StorageVolumeDto } from '@/types/generated/StorageVolumeDto'

export const useStorageStore = defineStore('storage', () => {
  const nodes = useNodesStore()
  /** Storage volumes per node id; `null` = failed to load. */
  const volumes = reactive(new Map<string, StorageVolumeDto[] | null>())

  async function loadForNode(nodeId: string) {
    volumes.set(nodeId, await nodeApi(nodeId)<StorageVolumeDto[]>('/storage').catch(() => null))
  }

  async function load() {
    for (const id of volumes.keys()) if (!nodes.reachable.some((n) => n.id === id)) volumes.delete(id)
    await Promise.all(nodes.reachable.map((n) => loadForNode(n.id)))
  }

  /** The volume with the least free space across every node. */
  const lowest = computed(() => {
    let best: { nodeId: string; volume: StorageVolumeDto } | null = null
    for (const [nodeId, list] of volumes) {
      for (const volume of list ?? []) {
        if (!best || volume.available_bytes < best.volume.available_bytes) best = { nodeId, volume }
      }
    }
    return best
  })

  return { volumes, load, loadForNode, lowest }
})
