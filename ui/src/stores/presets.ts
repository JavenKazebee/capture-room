import { defineStore } from 'pinia'
import { ref } from 'vue'
import { useApi } from '@/composables/useApi'

export interface Preset {
  id: string
  name: string
  codec: string
  container: string
  resolution: string | null
  framerate: string | null
  bitrate_kbps: number | null
  quality: string | null
  output_template: string
  secondary_output_template: string | null
  redundant_output_template: string | null
  created_at: string
  updated_at: string
  version: number
}

/** Editable fields — the server owns id, timestamps, and version. */
export type PresetInput = Omit<Preset, 'id' | 'created_at' | 'updated_at' | 'version'>

export const usePresetsStore = defineStore('presets', () => {
  const presets = ref<Preset[]>([])

  function set(list: Preset[]) {
    presets.value = list
  }

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

  return { presets, set, upsert, load, create, update, remove }
})
