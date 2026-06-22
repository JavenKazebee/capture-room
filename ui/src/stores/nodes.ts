import { defineStore } from 'pinia'
import { ref } from 'vue'

export interface NodeInfo {
  id: string
  name: string
  ip: string
  port: number
  status: 'online' | 'offline' | 'unknown'
  last_seen: string | null
  discovered: boolean
}

export const useNodesStore = defineStore('nodes', () => {
  const nodes = ref<NodeInfo[]>([])

  function upsert(node: NodeInfo) {
    const idx = nodes.value.findIndex((n) => n.id === node.id)
    if (idx === -1) nodes.value.push(node)
    else nodes.value[idx] = node
  }

  function setStatus(id: string, status: NodeInfo['status']) {
    const node = nodes.value.find((n) => n.id === id)
    if (node) node.status = status
  }

  function remove(id: string) {
    nodes.value = nodes.value.filter((n) => n.id !== id)
  }

  return { nodes, upsert, setStatus, remove }
})
