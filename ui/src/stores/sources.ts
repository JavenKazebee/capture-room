import { defineStore } from 'pinia'
import { ref } from 'vue'

export interface Source {
  id: string
  node_id: string
  display_name: string
  source_type: string
  is_available: boolean
  timecode: string | null
}

export const useSourcesStore = defineStore('sources', () => {
  const sources = ref<Source[]>([])

  function upsert(source: Source) {
    const idx = sources.value.findIndex((s) => s.id === source.id && s.node_id === source.node_id)
    if (idx === -1) sources.value.push(source)
    else sources.value[idx] = source
  }

  function remove(nodeId: string, sourceId: string) {
    sources.value = sources.value.filter((s) => !(s.node_id === nodeId && s.id === sourceId))
  }

  function removeByNode(nodeId: string) {
    sources.value = sources.value.filter((s) => s.node_id !== nodeId)
  }

  return { sources, upsert, remove, removeByNode }
})
