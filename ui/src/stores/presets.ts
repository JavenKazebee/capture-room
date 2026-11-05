import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api } from '@/composables/useApi'
import { defaultAdvanced } from '@/lib/advanced'
import type { PresetCreateRequest } from '@/types/generated/PresetCreateRequest'
import type { PresetDto } from '@/types/generated/PresetDto'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'
import { loadState } from '@/lib/loadState'
import { onReconnect } from '@/lib/reconnect'

/** What history calls a recording or benchmark that carries no preset name: one made with the H.264 preset every database starts with. */
export const UNNAMED_PRESET = 'H.264'

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
    path_template: '~/capture-room/{date}/{source_name}_{datetime}.{ext}',
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

  const { loaded, error, track } = loadState('presets')

  async function load() {
    await track(async () => (presets.value = await api<PresetDto[]>('/presets')))
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
    const wasDefault = id === defaultId.value
    await api(`/presets/${id}`, { method: 'DELETE' })
    presets.value = presets.value.filter((p) => p.id !== id)
    // The node moved the default to another preset.
    if (wasDefault) await load()
  }

  async function setDefault(id: string) {
    presets.value = await api<PresetDto[]>(`/presets/${id}/default`, { method: 'POST' })
  }

  /** Used where nothing else names a preset, such as a feed with none chosen. */
  const defaultPreset = computed(() => presets.value.find((p) => p.is_default) ?? presets.value[0] ?? null)
  const defaultId = computed(() => defaultPreset.value?.id ?? null)

  /** Every preset as select options. */
  const options = computed(() => presets.value.map((p) => ({ value: p.id, label: p.name })))

  return { presets, loaded, error, options, defaultPreset, defaultId, upsert, load, create, update, remove, setDefault }
})
