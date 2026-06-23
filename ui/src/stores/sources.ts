import { defineStore } from 'pinia'
import { ref, shallowReactive } from 'vue'

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
  timecode: TimecodeDto | null
  capabilities: SourceCapabilities
  node_id?: string
}

export interface ChannelLevel {
  peak_db: number
  rms_db: number
}

// Audio levels updated ~10fps — shallow to avoid deep reactivity overhead
export const audioLevels = shallowReactive(new Map<string, ChannelLevel[]>())

// Thumbnail cache-bust counter incremented on each thumbnail.updated event
export const thumbnailSeqs = shallowReactive(new Map<string, number>())

export const useSourcesStore = defineStore('sources', () => {
  const sources = ref<Source[]>([])

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
      s.timecode = s.timecode
        ? { ...s.timecode, display: tc }
        : null
    }
  }

  return { sources, upsert, remove, updateTimecode }
})
