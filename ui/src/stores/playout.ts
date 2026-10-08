import { defineStore } from 'pinia'
import { computed, reactive } from 'vue'
import { nodeApi, sourceKey } from '@/composables/useApi'
import { useSourcesStore } from '@/stores/sources'
import type { ChannelStatusDto } from '@/types/generated/ChannelStatusDto'
import type { MediaItemDto } from '@/types/generated/MediaItemDto'
import type { PlaylistDto } from '@/types/generated/PlaylistDto'
import type { PlaylistItemInput } from '@/types/generated/PlaylistItemInput'
import type { TransportAction } from '@/types/generated/TransportAction'

/**
 * Playout: each channel's transport (from `channel.state` events and the
 * replies to commands), its playlist (fetched when the status says it
 * changed) and each node's media library. A channel is a source of type
 * `channel`, keyed like any source.
 */
export const usePlayoutStore = defineStore('playout', () => {
  const sources = useSourcesStore()
  /** By source key. */
  const status = reactive(new Map<string, ChannelStatusDto>())
  /** By source key, for channels whose playlist was asked for. */
  const playlists = reactive(new Map<string, PlaylistDto>())
  /** By node id. */
  const media = reactive(new Map<string, MediaItemDto[]>())

  const channels = computed(() => sources.sources.filter((s) => s.source_type === 'channel'))

  function setStatus(nodeId: string, sourceId: string, s: ChannelStatusDto) {
    const key = sourceKey(nodeId, sourceId)
    status.set(key, s)
    const p = playlists.get(key)
    if (p && p.rev !== s.playlist_rev) loadPlaylist(nodeId, sourceId).catch(() => {})
  }

  async function loadPlaylist(nodeId: string, sourceId: string) {
    playlists.set(sourceKey(nodeId, sourceId), await nodeApi(nodeId)<PlaylistDto>(`/channels/${sourceId}/playlist`))
  }

  /** Replace a channel's playlist: the whole list, in order. */
  async function savePlaylist(nodeId: string, sourceId: string, items: PlaylistItemInput[], loopPlaylist: boolean) {
    const p = await nodeApi(nodeId)<PlaylistDto>(`/channels/${sourceId}/playlist`, {
      method: 'PUT',
      body: { items, loop_playlist: loopPlaylist },
    })
    playlists.set(sourceKey(nodeId, sourceId), p)
    return p
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

  /** Cue a playlist item: loaded, paused at its in point. */
  async function cue(nodeId: string, sourceId: string, itemId: string) {
    const s = await nodeApi(nodeId)<ChannelStatusDto>(`/channels/${sourceId}/cue`, {
      method: 'POST',
      body: { item_id: itemId },
    })
    setStatus(nodeId, sourceId, s)
  }

  async function transport(nodeId: string, sourceId: string, action: TransportAction, positionMs?: number) {
    const s = await nodeApi(nodeId)<ChannelStatusDto>(`/channels/${sourceId}/transport`, {
      method: 'POST',
      body: { action, position_ms: positionMs ?? null },
    })
    setStatus(nodeId, sourceId, s)
  }

  return {
    status,
    playlists,
    media,
    channels,
    setStatus,
    loadStatus,
    loadPlaylist,
    savePlaylist,
    loadMedia,
    addMedia,
    removeMedia,
    cue,
    transport,
  }
})
