import { defineStore } from 'pinia'
import { ref } from 'vue'
import { useApi } from '@/composables/useApi'

export interface OutputLeg {
  id: string
  preset_id: string
  name: string
  codec: string
  container: string
  resolution: string | null
  framerate: string | null
  bitrate_kbps: number | null
  quality: string | null
  path_template: string
  sort_order: number
}

export interface Preset {
  id: string
  name: string
  outputs: OutputLeg[]
  created_at: string
  updated_at: string
  version: number
}

export interface OutputLegInput {
  name: string
  codec: string
  container: string
  resolution: string | null
  framerate: string | null
  bitrate_kbps: number | null
  quality: string | null
  path_template: string
}

export interface PresetInput {
  name: string
  outputs: OutputLegInput[]
}

export function blankLeg(): OutputLegInput {
  return {
    name: 'Output',
    codec: 'h264',
    container: 'mov',
    resolution: null,
    framerate: null,
    bitrate_kbps: 8000,
    quality: null,
    path_template: '/tmp/capture-room/{source}_{datetime}.{ext}',
  }
}

export const usePresetsStore = defineStore('presets', () => {
  const presets = ref<Preset[]>([])

  function upsert(preset: Preset) {
    const idx = presets.value.findIndex((p) => p.id === preset.id)
    if (idx === -1) presets.value.push(preset)
    else presets.value[idx] = preset
  }

  async function load() {
    const { api } = useApi()
    presets.value = await api<Preset[]>('/presets').catch(() => [])
  }

  async function create(input: PresetInput): Promise<Preset> {
    const { api } = useApi()
    const p = await api<Preset>('/presets', { method: 'POST', body: input })
    upsert(p)
    return p
  }

  async function update(id: string, input: PresetInput): Promise<Preset> {
    const { api } = useApi()
    const p = await api<Preset>(`/presets/${id}`, { method: 'PUT', body: input })
    upsert(p)
    return p
  }

  async function remove(id: string) {
    const { api } = useApi()
    await api(`/presets/${id}`, { method: 'DELETE' })
    presets.value = presets.value.filter((p) => p.id !== id)
  }

  return { presets, upsert, load, create, update, remove }
})
