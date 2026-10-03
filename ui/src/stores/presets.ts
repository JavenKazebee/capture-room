import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api } from '@/composables/useApi'
import type { PresetCreateRequest } from '@/types/generated/PresetCreateRequest'
import type { PresetDto } from '@/types/generated/PresetDto'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'

export function blankLeg(): PresetOutputInput {
  return {
    name: 'Output',
    codec: 'h264',
    container: 'mov',
    resolution: null,
    framerate: null,
    bitrate_kbps: 8000,
    chroma: '420',
    path_template: '/tmp/capture-room/{source}_{datetime}.{ext}',
  }
}

/** A preset's output legs as editable / sendable inputs (stored ids dropped). */
export function presetLegs(preset: PresetDto): PresetOutputInput[] {
  return preset.outputs.map(({ id: _id, preset_id: _preset, sort_order: _order, ...leg }) => leg)
}

export const usePresetsStore = defineStore('presets', () => {
  const presets = ref<PresetDto[]>([])

  function upsert(preset: PresetDto) {
    const idx = presets.value.findIndex((p) => p.id === preset.id)
    if (idx === -1) presets.value.push(preset)
    else presets.value[idx] = preset
  }

  async function load() {
    presets.value = await api<PresetDto[]>('/presets').catch(() => [])
  }

  async function create(input: PresetCreateRequest): Promise<PresetDto> {
    const p = await api<PresetDto>('/presets', { method: 'POST', body: input })
    upsert(p)
    return p
  }

  async function update(id: string, input: PresetCreateRequest): Promise<PresetDto> {
    const p = await api<PresetDto>(`/presets/${id}`, { method: 'PUT', body: input })
    upsert(p)
    return p
  }

  async function remove(id: string) {
    await api(`/presets/${id}`, { method: 'DELETE' })
    presets.value = presets.value.filter((p) => p.id !== id)
  }

  return { presets, upsert, load, create, update, remove }
})
