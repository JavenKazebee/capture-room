import { defineStore } from 'pinia'
import { ref, shallowReactive } from 'vue'
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

// Audio levels updated ~10fps — shallow to avoid deep reactivity overhead
export const audioLevels = shallowReactive(new Map<string, ChannelLevel[]>())

// Thumbnail cache-bust counter incremented on each thumbnail.updated event
export const thumbnailSeqs = shallowReactive(new Map<string, number>())

export const useSourcesStore = defineStore('sources', () => {
  const { api } = useApi()

  const sources = ref<Source[]>([])
  const testConfigs = ref<TestSourceConfig[]>([])

  function upsert(source: Source) {
    const idx = sources.value.findIndex((s) => s.id === source.id)
    if (idx === -1) sources.value.push(source)
    else sources.value[idx] = source
  }

  function remove(id: string) {
    sources.value = sources.value.filter((s) => s.id !== id)
  }

  function updateTimecode(sourceId: string, tc: string | null) {
    const s = sources.value.find((s) => s.id === sourceId)
    if (s && tc !== null) {
      s.timecode = s.timecode ? { ...s.timecode, display: tc } : null
    }
  }

  async function loadSources() {
    sources.value = await api<Source[]>('/sources')
  }

  async function loadTestConfigs() {
    testConfigs.value = await api<TestSourceConfig[]>('/sources/test')
  }

  async function createTestSource(input: TestSourceInput, nodeId?: string): Promise<TestSourceConfig> {
    const query = nodeId ? `?node_id=${encodeURIComponent(nodeId)}` : ''
    const created = await api<TestSourceConfig>(`/sources/test${query}`, {
      method: 'POST',
      body: input,
    })
    await Promise.all([loadTestConfigs(), loadSources()])
    return created
  }

  async function updateTestSource(id: string, input: TestSourceInput, nodeId?: string): Promise<TestSourceConfig> {
    const query = nodeId ? `?node_id=${encodeURIComponent(nodeId)}` : ''
    const updated = await api<TestSourceConfig>(`/sources/test/${id}${query}`, {
      method: 'PUT',
      body: input,
    })
    const idx = testConfigs.value.findIndex((c) => c.id === id)
    if (idx !== -1) testConfigs.value[idx] = updated
    await loadSources()
    return updated
  }

  async function deleteTestSource(id: string, nodeId?: string) {
    const query = nodeId ? `?node_id=${encodeURIComponent(nodeId)}` : ''
    await api(`/sources/test/${id}${query}`, { method: 'DELETE' })
    testConfigs.value = testConfigs.value.filter((c) => c.id !== id)
    await loadSources()
  }

  async function scan() {
    const updated = await api<Source[]>('/sources/scan', { method: 'POST' })
    sources.value = updated
  }

  return {
    sources,
    testConfigs,
    upsert,
    remove,
    updateTimecode,
    loadSources,
    loadTestConfigs,
    createTestSource,
    updateTestSource,
    deleteTestSource,
    scan,
  }
})
