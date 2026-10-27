import { defineStore } from 'pinia'
import { computed, reactive } from 'vue'
import { FetchError } from 'ofetch'
import { nodeApi } from '@/composables/useApi'
import { useNodesStore } from '@/stores/nodes'
import { nodeLoadErrors } from '@/lib/loadState'
import { onReconnect } from '@/lib/reconnect'
import type { StorageVolumeDto } from '@/types/generated/StorageVolumeDto'
import type { CreateFolderRequest } from '@/types/generated/CreateFolderRequest'
import type { FolderDto } from '@/types/generated/FolderDto'
import type { FolderListingDto } from '@/types/generated/FolderListingDto'

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

  /** A folder's subfolders on a node; with no `path`, where to start: home, the storage folder and the volumes. */
  function listFolders(nodeId: string, path?: string) {
    return nodeApi(nodeId)<FolderListingDto>('/fs/dirs', { query: path ? { path } : {} })
  }

  function createFolder(nodeId: string, parent: string, name: string) {
    const body: CreateFolderRequest = { parent, name }
    return nodeApi(nodeId)<FolderDto>('/fs/dirs', { method: 'POST', body })
  }

  /** Whether `path` is a folder on the node; `null` when it can't tell (unreachable, unreadable). */
  async function folderExists(nodeId: string, path: string): Promise<boolean | null> {
    try {
      await listFolders(nodeId, path)
      return true
    } catch (e) {
      return e instanceof FetchError && e.statusCode === 404 ? false : null
    }
  }

  return { volumes, errors, load, loadForNode, lowest, soonestFull, listFolders, createFolder, folderExists }
})
