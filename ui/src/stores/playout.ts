import { defineStore } from 'pinia'
import { computed, reactive } from 'vue'
import { nodeApi, sourceKey } from '@/composables/useApi'
import { useSourcesStore } from '@/stores/sources'
import type { ChannelStatusDto } from '@/types/generated/ChannelStatusDto'
import type { ClipEnd } from '@/types/generated/ClipEnd'
import type { MediaItemDto } from '@/types/generated/MediaItemDto'
import type { TransportAction } from '@/types/generated/TransportAction'

/**
 * Playout: each channel's transport (from `channel.state` events and the
 * replies to commands) and each node's media library. A channel is a source
 * of type `channel`, keyed like any source.
 */
export const usePlayoutStore = defineStore('playout', () => {
  const sources = useSourcesStore()
  /** By source key. */
  const status = reactive(new Map<string, ChannelStatusDto>())
  /** By node id. */
  const media = reactive(new Map<string, MediaItemDto[]>())

  const channels = computed(() => sources.sources.filter((s) => s.source_type === 'channel'))

  function setStatus(nodeId: string, sourceId: string, s: ChannelStatusDto) {
    status.set(sourceKey(nodeId, sourceId), s)
  }

  async function loadStatus(nodeId: string, sourceId: string) {
    setStatus(nodeId, sourceId, await nodeApi(nodeId)<ChannelStatusDto>(`/channels/${sourceId}`))
  }

  async function loadMedia(nodeId: string) {
    media.set(nodeId, await nodeApi(nodeId)<MediaItemDto[]>('/media'))
  }

  async function addMedia(nodeId: string, path: string, sessionId?: string) {
    const item = await nodeApi(nodeId)<MediaItemDto>('/media', {
      method: 'POST',
      body: { path, session_id: sessionId ?? null },
    })
    await loadMedia(nodeId)
    return item
  }

  async function removeMedia(nodeId: string, id: string) {
    await nodeApi(nodeId)(`/media/${id}`, { method: 'DELETE' })
    await loadMedia(nodeId)
  }

  /** Cue a clip on a channel: loaded, paused at its in point. */
  async function load(
    nodeId: string,
    sourceId: string,
    clip: { media_id: string; in_ms: number | null; out_ms: number | null; end: ClipEnd },
  ) {
    const s = await nodeApi(nodeId)<ChannelStatusDto>(`/channels/${sourceId}/load`, { method: 'POST', body: clip })
    setStatus(nodeId, sourceId, s)
  }

  async function transport(nodeId: string, sourceId: string, action: TransportAction, positionMs?: number) {
    const s = await nodeApi(nodeId)<ChannelStatusDto>(`/channels/${sourceId}/transport`, {
      method: 'POST',
      body: { action, position_ms: positionMs ?? null },
    })
    setStatus(nodeId, sourceId, s)
  }

  return { status, media, channels, setStatus, loadStatus, loadMedia, addMedia, removeMedia, load, transport }
})
