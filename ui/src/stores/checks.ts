import { defineStore } from 'pinia'
import { reactive } from 'vue'
import { api, nodeApi } from '@/composables/useApi'
import { useNodesStore } from '@/stores/nodes'
import { onReconnect } from '@/lib/reconnect'
import type { CheckDto } from '@/types/generated/CheckDto'
import type { NodeChecksDto } from '@/types/generated/NodeChecksDto'
import type { NodeDto } from '@/types/generated/NodeDto'

/** Missing, and something on the node needs it now. */
export function isProblem(check: CheckDto) {
  return check.status === 'missing' && check.required
}

function isLoopback(host: string) {
  return host === 'localhost' || host === '[::1]' || host === '::1' || host.startsWith('127.')
}

/**
 * Whether this browser runs on `node`'s machine, so a System Settings link or
 * a permission prompt would open in front of the person clicking it.
 */
export function browserIsOn(node: NodeDto) {
  if (!isLoopback(window.location.hostname)) return false
  if (node.is_self) return true
  try {
    return isLoopback(new URL(node.url).hostname)
  } catch {
    return false
  }
}

/**
 * Setup checks per node, read live from each node (nothing is stored), plus
 * the controller's probes of each node's ports.
 */
export const useChecksStore = defineStore('checks', () => {
  const nodes = useNodesStore()
  /** `undefined` while loading, `null` if it failed. */
  const byNode = reactive(new Map<string, NodeChecksDto | null>())
  /** Port probes from the controller; absent when not a controller. */
  const reachability = reactive(new Map<string, CheckDto[] | null>())

  async function loadForNode(nodeId: string) {
    const probe = nodes.isController
      ? api<CheckDto[]>(`/reachability/${encodeURIComponent(nodeId)}`).catch(() => null)
      : Promise.resolve(undefined)
    const [checks, ports] = await Promise.all([
      nodeApi(nodeId)<NodeChecksDto>('/checks').catch(() => null),
      probe,
    ])
    byNode.set(nodeId, checks)
    if (ports === undefined) reachability.delete(nodeId)
    else reachability.set(nodeId, ports)
  }

  async function load() {
    for (const id of byNode.keys()) {
      if (!nodes.reachable.some((n) => n.id === id)) {
        byNode.delete(id)
        reachability.delete(id)
      }
    }
    await Promise.all(nodes.reachable.map((n) => loadForNode(n.id)))
  }

  onReconnect(load)

  /** Every check on a node, its own and the controller's probes. */
  function all(nodeId: string): CheckDto[] {
    return [...(byNode.get(nodeId)?.checks ?? []), ...(reachability.get(nodeId) ?? [])]
  }

  function problems(nodeId: string) {
    return all(nodeId).filter(isProblem).length
  }

  /** Ask the node's OS to show its prompt for a permission. */
  async function request(nodeId: string, id: string) {
    await nodeApi(nodeId)('/checks/request', { method: 'POST', body: { id } })
  }

  return { byNode, reachability, load, loadForNode, all, problems, request }
})
