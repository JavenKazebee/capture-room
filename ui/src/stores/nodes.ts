import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { useApi } from '@/composables/useApi'

export interface NodeInfo {
  id: string
  name: string
  /** Empty for this instance. */
  url: string
  version: string
  healthy: boolean
  uptime_secs: number
  is_self: boolean
  /** Added by URL (persisted) rather than discovered via mDNS. */
  manual: boolean
}

export interface MonitorSettings {
  thumb_fps: number
  thumb_width: number
  thumb_height: number
  level_interval_ms: number
}

export interface NodeSettings {
  node_id: string
  node_name: string
  is_controller: boolean
  monitor: MonitorSettings
}

export interface StorageVolume {
  name: string
  mount_point: string
  file_system: string
  total_bytes: number
  available_bytes: number
  removable: boolean
}

export const useNodesStore = defineStore('nodes', () => {
  const { api } = useApi()

  const nodes = ref<NodeInfo[]>([])
  /** Settings of the instance the UI is connected to. */
  const self = ref<NodeSettings | null>(null)

  const isController = computed(() => self.value?.is_controller ?? false)
  /** Nodes that can currently be reached. */
  const reachable = computed(() => nodes.value.filter((n) => n.healthy))

  async function load() {
    const [list, settings] = await Promise.all([
      api<NodeInfo[]>('/nodes').catch(() => [] as NodeInfo[]),
      api<NodeSettings>('/node/settings').catch(() => null),
    ])
    nodes.value = list
    self.value = settings
  }

  async function setController(enabled: boolean) {
    await api('/controller', { method: 'PUT', body: { enabled } })
    await load()
  }

  async function add(url: string) {
    await api('/nodes', { method: 'POST', body: { url } })
    await load()
  }

  async function remove(id: string) {
    await api(`/nodes/${encodeURIComponent(id)}`, { method: 'DELETE' })
    await load()
  }

  function nameOf(id: string) {
    return nodes.value.find((n) => n.id === id)?.name ?? id.slice(0, 8)
  }

  return { nodes, self, isController, reachable, load, setController, add, remove, nameOf }
})
