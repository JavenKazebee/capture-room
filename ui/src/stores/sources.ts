import { defineStore } from 'pinia'
import { ref, shallowReactive } from 'vue'
import { nodeApi, sourceKey } from '@/composables/useApi'
import { useNodesStore } from '@/stores/nodes'
import type { ChannelLevelDto } from '@/types/generated/ChannelLevelDto'
import type { TestSourceRequest } from '@/types/generated/TestSourceRequest'
import type { SourceDto } from '@/types/generated/SourceDto'
import type { TestSourceConfigDto } from '@/types/generated/TestSourceConfigDto'

/** A node's source tagged with the node it lives on (`id` is only unique per node). */
export type Source = SourceDto & {
  node_id: string
  /** `${node_id}/${id}` — unique across nodes. */
  key: string
}

// Audio levels updated ~10fps — shallow to avoid deep reactivity overhead.
// Both maps are keyed by sourceKey(node_id, source_id).
export const audioLevels = shallowReactive(new Map<string, ChannelLevelDto[]>())

// Thumbnail cache-bust counter incremented on each thumbnail.updated event
export const thumbnailSeqs = shallowReactive(new Map<string, number>())

export const useSourcesStore = defineStore('sources', () => {
  const nodes = useNodesStore()
  const sources = ref<Source[]>([])

  function tag(nodeId: string, list: SourceDto[]): Source[] {
    return list.map((s) => ({ ...s, node_id: nodeId, key: sourceKey(nodeId, s.id) }))
  }

  function updateTimecode(nodeId: string, sourceId: string, tc: string | null) {
    const key = sourceKey(nodeId, sourceId)
    const s = sources.value.find((s) => s.key === key)
    if (s && tc !== null) {
      s.timecode = s.timecode ? { ...s.timecode, display: tc } : null
    }
  }

  /** Replace one node's sources, keeping the others. */
  function setForNode(nodeId: string, list: SourceDto[]) {
    sources.value = [...sources.value.filter((s) => s.node_id !== nodeId), ...tag(nodeId, list)]
  }

  async function loadSources() {
    if (nodes.nodes.length === 0) await nodes.load()
    const results = await Promise.all(
      nodes.reachable.map(async (n) =>
        tag(n.id, await nodeApi(n.id)<SourceDto[]>('/sources').catch(() => [])),
      ),
    )
    sources.value = results.flat()
  }

  async function scan(nodeId: string) {
    setForNode(nodeId, await nodeApi(nodeId)<SourceDto[]>('/sources/scan', { method: 'POST' }))
  }

  async function scanAll() {
    await Promise.all(nodes.reachable.map((n) => scan(n.id)))
  }

  async function testConfigs(nodeId: string) {
    return nodeApi(nodeId)<TestSourceConfigDto[]>('/test-sources')
  }

  async function createTestSource(nodeId: string, input: TestSourceRequest) {
    const created = await nodeApi(nodeId)<TestSourceConfigDto>('/test-sources', {
      method: 'POST',
      body: input,
    })
    setForNode(nodeId, await nodeApi(nodeId)<SourceDto[]>('/sources'))
    return created
  }

  async function updateTestSource(nodeId: string, id: string, input: TestSourceRequest) {
    const updated = await nodeApi(nodeId)<TestSourceConfigDto>(`/test-sources/${id}`, {
      method: 'PUT',
      body: input,
    })
    setForNode(nodeId, await nodeApi(nodeId)<SourceDto[]>('/sources'))
    return updated
  }

  async function deleteTestSource(nodeId: string, id: string) {
    await nodeApi(nodeId)(`/test-sources/${id}`, { method: 'DELETE' })
    setForNode(nodeId, await nodeApi(nodeId)<SourceDto[]>('/sources'))
  }

  return {
    sources,
    updateTimecode,
    loadSources,
    scan,
    scanAll,
    testConfigs,
    createTestSource,
    updateTestSource,
    deleteTestSource,
  }
})
