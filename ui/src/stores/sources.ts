import { defineStore } from 'pinia'
import { ref, shallowReactive } from 'vue'
import { useApi } from '@/composables/useApi'

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

export interface Source {
  id: string
  display_name: string
  source_type: string
  is_available: boolean
  connected: boolean
  timecode: TimecodeDto | null
  capabilities: SourceCapabilities
  node_id?: string
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
