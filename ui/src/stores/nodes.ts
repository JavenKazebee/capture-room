import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api, nodeApi } from '@/composables/useApi'
import type { NodeDto } from '@/types/generated/NodeDto'
import type { NodeSettingsDto } from '@/types/generated/NodeSettingsDto'
import type { AddNodeRequest } from '@/types/generated/AddNodeRequest'
import type { FoundNodeDto } from '@/types/generated/FoundNodeDto'
import type { ControllerToggleRequest } from '@/types/generated/ControllerToggleRequest'
import type { UpdateNodeSettingsRequest } from '@/types/generated/UpdateNodeSettingsRequest'

/**
 * A node's address as other machines reach its media: its `url`, else the UI's own address.
 * `origin` is plain HTTP, where players reach `/whep` without trusting the node's certificate.
 */
export function nodeAddress(url: string | null | undefined): { hostname: string; origin: string } {
  let at: { hostname: string; host: string } = window.location
  if (url) {
    try {
      at = new URL(url)
    } catch {
      // Fall back to the UI's own address.
    }
  }
  return { hostname: at.hostname, origin: `http://${at.host}` }
}

export const useNodesStore = defineStore('nodes', () => {
  const nodes = ref<NodeDto[]>([])
  /** Nodes that announced themselves and wait for this controller to approve them. */
  const found = ref<FoundNodeDto[]>([])
  /** Settings of the instance the UI is connected to. */
  const self = ref<NodeSettingsDto | null>(null)
  /** Names just set by `rename`, held over reloads until the controller reports them. */
  const renamed = new Map<string, { name: string; until: number }>()

  const isController = computed(() => self.value?.is_controller ?? false)
  /** Nodes that can currently be reached. */
  const reachable = computed(() => nodes.value.filter((n) => n.healthy))

  async function load() {
    const [list, settings, waiting] = await Promise.all([
      api<NodeDto[]>('/nodes').catch(() => [] as NodeDto[]),
      api<NodeSettingsDto>('/node/settings').catch(() => null),
      api<FoundNodeDto[]>('/found-nodes').catch(() => [] as FoundNodeDto[]),
    ])
    found.value = waiting
    for (const n of list) {
      const r = renamed.get(n.id)
      if (!r) continue
      if (n.name === r.name || Date.now() > r.until) renamed.delete(n.id)
      else n.name = r.name
    }
    nodes.value = list
    self.value = settings
  }

  async function setController(enabled: boolean) {
    await api('/controller', { method: 'PUT', body: { enabled } satisfies ControllerToggleRequest })
    await load()
  }

  async function add(url: string) {
    await api('/nodes', { method: 'POST', body: { url } satisfies AddNodeRequest })
    await load()
  }

  async function approve(id: string) {
    await api(`/found-nodes/${encodeURIComponent(id)}/approve`, { method: 'POST' })
    await load()
  }

  /** Not offered again until the controller restarts. */
  async function dismiss(id: string) {
    await api(`/found-nodes/${encodeURIComponent(id)}`, { method: 'DELETE' })
    await load()
  }

  async function remove(id: string) {
    await api(`/nodes/${encodeURIComponent(id)}`, { method: 'DELETE' })
    await load()
  }

  /**
   * Applied locally at once: a controller only picks up a peer's new name on its next
   * health check (~5s), so a `node.updated` reload would show the old one until then.
   */
  async function rename(id: string, name: string) {
    const settings = await nodeApi(id)<NodeSettingsDto>('/settings', {
      method: 'PUT',
      body: { name, monitor: null } satisfies UpdateNodeSettingsRequest,
    })
    renamed.set(id, { name: settings.node_name, until: Date.now() + 15_000 })
    const node = nodes.value.find((n) => n.id === id)
    if (node) node.name = settings.node_name
    if (self.value?.node_id === id) self.value = settings
  }

  function nameOf(id: string) {
    return nodes.value.find((n) => n.id === id)?.name ?? id.slice(0, 8)
  }

  /** Name for pickers and headings, marking the instance the UI is talking to. */
  function labelOf(id: string) {
    const name = nameOf(id)
    return id === self.value?.node_id ? `${name} (this node)` : name
  }

  return { nodes, found, self, isController, reachable, load, setController, add, approve, dismiss, remove, rename, nameOf, labelOf }
})
