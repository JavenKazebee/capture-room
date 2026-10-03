import { defineStore } from 'pinia'
import { ref, shallowReactive } from 'vue'
<<<<<<< HEAD
import { nodeApi, sourceKey } from '@/composables/useApi'
import { useNodesStore } from '@/stores/nodes'

export interface TimecodeDto {
  hours: number
  minutes: number
  seconds: number
  frames: number
  drop_frame: boolean
  framerate: [number, number]
  display: string
}

export interface SourceCapabilities {
  video_formats: string[]
  max_width: number
  max_height: number
  max_framerate: [number, number]
  audio_channels: number
  audio_sample_rates: number[]
}

/** As returned by a node: `id` is only unique within that node. */
export interface SourceDto {
  id: string
  display_name: string
  source_type: string
  is_available: boolean
  connected: boolean
  timecode: TimecodeDto | null
  capabilities: SourceCapabilities
}

export interface Source extends SourceDto {
  node_id: string
  /** `${node_id}/${id}` — unique across nodes. */
  key: string
}

export interface ChannelLevel {
  peak_db: number
  rms_db: number
}

export interface TestSourceConfig {
  id: string
  name: string
  pattern: string
  width: number
  height: number
  fps_num: number
  fps_den: number
  audio_signal: string
  frequency: number
  channels: number
  created_at: string
}

export type TestSourceInput = Omit<TestSourceConfig, 'id' | 'created_at'>
=======
import { useApi } from '@/composables/useApi'
import type { ChannelLevelDto } from '@/types/generated/ChannelLevelDto'
import type { CreateTestSourceRequest } from '@/types/generated/CreateTestSourceRequest'
import type { SourceCapabilitiesDto } from '@/types/generated/SourceCapabilitiesDto'
import type { SourceDto } from '@/types/generated/SourceDto'
import type { TestSourceConfigDto } from '@/types/generated/TestSourceConfigDto'

export type { TimecodeDto } from '@/types/generated/TimecodeDto'
export type SourceCapabilities = SourceCapabilitiesDto
// node_id is added by the aggregator when proxying a remote node's sources
export type Source = SourceDto & { node_id?: string }
export type ChannelLevel = ChannelLevelDto
export type TestSourceConfig = TestSourceConfigDto
export type TestSourceInput = CreateTestSourceRequest
>>>>>>> claude/lucid-bun-9f1bf1

// Audio levels updated ~10fps — shallow to avoid deep reactivity overhead.
// Both maps are keyed by sourceKey(node_id, source_id).
export const audioLevels = shallowReactive(new Map<string, ChannelLevel[]>())

// Thumbnail cache-bust counter incremented on each thumbnail.updated event
export const thumbnailSeqs = shallowReactive(new Map<string, number>())

export const useSourcesStore = defineStore('sources', () => {
  const nodes = useNodesStore()
  const sources = ref<Source[]>([])

  function tag(nodeId: string, list: SourceDto[]): Source[] {
    return list.map((s) => ({ ...s, node_id: nodeId, key: sourceKey(nodeId, s.id) }))
  }

  function remove(nodeId: string, sourceId: string) {
    const key = sourceKey(nodeId, sourceId)
    sources.value = sources.value.filter((s) => s.key !== key)
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
    return nodeApi(nodeId)<TestSourceConfig[]>('/test-sources')
  }

  async function createTestSource(nodeId: string, input: TestSourceInput) {
    const created = await nodeApi(nodeId)<TestSourceConfig>('/test-sources', {
      method: 'POST',
      body: input,
    })
    setForNode(nodeId, await nodeApi(nodeId)<SourceDto[]>('/sources'))
    return created
  }

  async function updateTestSource(nodeId: string, id: string, input: TestSourceInput) {
    const updated = await nodeApi(nodeId)<TestSourceConfig>(`/test-sources/${id}`, {
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
    remove,
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
