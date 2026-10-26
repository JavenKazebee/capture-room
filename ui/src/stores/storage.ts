import { defineStore } from 'pinia'
import { computed, reactive } from 'vue'
import { nodeApi } from '@/composables/useApi'
import { useNodesStore } from '@/stores/nodes'
import { nodeLoadErrors } from '@/lib/loadState'
import { onReconnect } from '@/lib/reconnect'
import type { StorageVolumeDto } from '@/types/generated/StorageVolumeDto'

export const useStorageStore = defineStore('storage', () => {
  const nodes = useNodesStore()
  /** Storage volumes per node id, the last good reading; a failed load keeps it and sets `errors`. */
  const volumes = reactive(new Map<string, StorageVolumeDto[]>())
  const { errors, track, prune } = nodeLoadErrors()

  async function loadForNode(nodeId: string) {
    const list = await track(nodeId, () => nodeApi(nodeId)<StorageVolumeDto[]>('/storage'))
    if (list) volumes.set(nodeId, list)
  }

  async function load() {
    for (const id of volumes.keys()) if (!nodes.reachable.some((n) => n.id === id)) volumes.delete(id)
    prune(nodes.reachable.map((n) => n.id))
    await Promise.all(nodes.reachable.map((n) => loadForNode(n.id)))
  }

  onReconnect(load)

  /** The volume with the least free space across every node. */
  const lowest = computed(() => {
    let best: { nodeId: string; volume: StorageVolumeDto } | null = null
    for (const [nodeId, list] of volumes) {
      for (const volume of list) {
        if (!best || volume.available_bytes < best.volume.available_bytes) best = { nodeId, volume }
      }
    }
    return best
  })

  /** The volume recordings will fill soonest, across every node. */
  const soonestFull = computed(() => {
    let best: { nodeId: string; volume: StorageVolumeDto } | null = null
    for (const [nodeId, list] of volumes) {
      for (const volume of list) {
        if (volume.seconds_left == null) continue
        if (!best || volume.seconds_left < best.volume.seconds_left!) best = { nodeId, volume }
      }
    }
    return best
  })

  return { volumes, errors, load, loadForNode, lowest, soonestFull }
})
