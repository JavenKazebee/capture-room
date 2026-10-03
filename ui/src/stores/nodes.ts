import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api } from '@/composables/useApi'
import type { NodeDto } from '@/types/generated/NodeDto'
import type { NodeSettingsDto } from '@/types/generated/NodeSettingsDto'

export const useNodesStore = defineStore('nodes', () => {
  const nodes = ref<NodeDto[]>([])
  /** Settings of the instance the UI is connected to. */
  const self = ref<NodeSettingsDto | null>(null)

  const isController = computed(() => self.value?.is_controller ?? false)
  /** Nodes that can currently be reached. */
  const reachable = computed(() => nodes.value.filter((n) => n.healthy))

  async function load() {
    const [list, settings] = await Promise.all([
      api<NodeDto[]>('/nodes').catch(() => [] as NodeDto[]),
      api<NodeSettingsDto>('/node/settings').catch(() => null),
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
