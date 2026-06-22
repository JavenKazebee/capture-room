import { defineStore } from 'pinia'
import { ref } from 'vue'

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

  function remove(id: string) {
    presets.value = presets.value.filter((p) => p.id !== id)
  }

  return { presets, set, upsert, remove }
})
