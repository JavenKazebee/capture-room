import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api } from '@/composables/useApi'
import { defaultAdvanced } from '@/lib/advanced'
import type { PresetCreateRequest } from '@/types/generated/PresetCreateRequest'
import type { PresetDto } from '@/types/generated/PresetDto'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'
import { onReconnect } from '@/lib/reconnect'

export function blankLeg(): PresetOutputInput {
  return {
    name: 'Output',
    codec: 'h264',
    container: 'mov',
    resolution: null,
    framerate: null,
    // null = Auto: sized by the node from the frame size and rate.
    bitrate_kbps: null,
    chroma: '420',
    // `~` is the recording node's home directory, expanded on the node.
    path_template: '~/capture-room/{date}/{source}_{datetime}.{ext}',
    advanced: defaultAdvanced(),
  }
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

  onReconnect(load)

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
