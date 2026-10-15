import { defineStore } from 'pinia'
import { reactive } from 'vue'
import { nodeApi, sourceKey } from '@/composables/useApi'
import { useNodesStore } from '@/stores/nodes'
import { usePlayoutStore } from '@/stores/playout'
import type { ChannelStatusDto } from '@/types/generated/ChannelStatusDto'
import type { ReplayBufferDto } from '@/types/generated/ReplayBufferDto'
import type { ReplayClipRequest } from '@/types/generated/ReplayClipRequest'
import type { ReplayCodec } from '@/types/generated/ReplayCodec'
import type { ReplayConfigInput } from '@/types/generated/ReplayConfigInput'
import type { ReplayClipDto } from '@/types/generated/ReplayClipDto'
import type { SaveReplayClipRequest } from '@/types/generated/SaveReplayClipRequest'
import type { MediaItemDto } from '@/types/generated/MediaItemDto'
import type { PlayReplayClipRequest } from '@/types/generated/PlayReplayClipRequest'
import type { UpdateReplayClipRequest } from '@/types/generated/UpdateReplayClipRequest'

export type ReplayBuffer = ReplayBufferDto & {
  node_id: string
  /** The source's key. */
  key: string
  /** `performance.now()` when it was fetched, to run its clock on from `now_us`. */
  fetched_at: number
}

export type ReplayClip = ReplayClipDto & {
  node_id: string
  /** `node_id/id`. */
  key: string
}

export const CODECS: Record<ReplayCodec, { label: string; note: string }> = {
  prores_proxy: { label: 'ProRes 422 Proxy', note: 'About 40 GB an hour at 1080p59.94. Mac only.' },
  h264_intra: {
    label: 'H.264, every frame a keyframe',
    note: "As good a picture as ProRes Proxy's at about 60% of the size: 20–30 GB an hour at 1080p59.94.",
  },
  mjpeg: { label: 'Motion JPEG', note: 'About 40–55 GB an hour at 1080p59.94. Any platform.' },
}

/**
 * Replay buffers on every node (polled while the Replay workspace is open:
 * their span moves every second) and the commands that play from them on a
 * channel. Channel status lands in the playout store, like Playback's.
 */
export const useReplayStore = defineStore('replay', () => {
  const nodes = useNodesStore()
  const playout = usePlayoutStore()
  /** By source key. */
  const buffers = reactive(new Map<string, ReplayBuffer>())
  /** Saved clips, by node, newest first. */
  const clips = reactive(new Map<string, ReplayClip[]>())

  async function load(nodeId: string) {
    const list = await nodeApi(nodeId)<ReplayBufferDto[]>('/replay')
    const now = performance.now()
    for (const [key, b] of buffers) if (b.node_id === nodeId) buffers.delete(key)
    for (const b of list) {
      const key = sourceKey(nodeId, b.source_id)
      buffers.set(key, { ...b, node_id: nodeId, key, fetched_at: now })
    }
  }

  async function loadAll() {
    if (!nodes.nodes.length) await nodes.load()
    await Promise.all(nodes.reachable.map((n) => load(n.id).catch(() => {})))
  }

  /** The codecs a node can keep buffers in, default first; empty if it isn't listed. */
  function codecsOf(nodeId: string): ReplayCodec[] {
    return nodes.nodes.find((n) => n.id === nodeId)?.replay_codecs ?? []
  }

  /** A buffer's node clock now (µs), run on from when it was fetched. */
  function nowUs(b: ReplayBuffer, at = performance.now()) {
    return b.now_us + Math.round((at - b.fetched_at) * 1000)
  }

  async function setBuffer(nodeId: string, sourceId: string, config: ReplayConfigInput) {
    await nodeApi(nodeId)(`/replay/${sourceId}`, { method: 'PUT', body: config })
    await load(nodeId)
  }

  async function removeBuffer(nodeId: string, sourceId: string) {
    await nodeApi(nodeId)(`/replay/${sourceId}`, { method: 'DELETE' })
    await load(nodeId)
  }

  /** Put part of a buffer on a channel (same node), cued or playing. */
  async function play(nodeId: string, channelId: string, req: ReplayClipRequest) {
    const s = await nodeApi(nodeId)<ChannelStatusDto>(`/channels/${channelId}/replay`, {
      method: 'POST',
      body: req,
    })
    playout.setStatus(nodeId, channelId, s)
    return s
  }


  // ── Saved clips ─────────────────────────────────────────────────────────────

  async function loadClips(nodeId: string) {
    const list = await nodeApi(nodeId)<ReplayClipDto[]>('/replay/clips')
    clips.set(
      nodeId,
      list.map((c) => ({ ...c, node_id: nodeId, key: `${nodeId}/${c.id}` })),
    )
  }


  async function saveClip(nodeId: string, req: SaveReplayClipRequest) {
    const c = await nodeApi(nodeId)<ReplayClipDto>('/replay/clips', { method: 'POST', body: req })
    await loadClips(nodeId)
    return c
  }

  async function renameClip(clip: ReplayClip, name: string) {
    await nodeApi(clip.node_id)(`/replay/clips/${clip.id}`, { method: 'PATCH', body: { name } satisfies UpdateReplayClipRequest })
    await loadClips(clip.node_id)
  }

  async function removeClip(clip: ReplayClip) {
    await nodeApi(clip.node_id)(`/replay/clips/${clip.id}`, { method: 'DELETE' })
    await loadClips(clip.node_id)
  }

  /** Remux a clip into a file in the node's Replay clips folder; it joins the media library. */
  async function exportClip(clip: ReplayClip) {
    return nodeApi(clip.node_id)<MediaItemDto>(`/replay/clips/${clip.id}/export`, { method: 'POST' })
  }

  /** Put a saved clip on a channel (same node), cued or playing. */
  async function playClip(channelId: string, clip: ReplayClip, rate: number, play: boolean) {
    const s = await nodeApi(clip.node_id)<ChannelStatusDto>(`/channels/${channelId}/replay-clip`, {
      method: 'POST',
      body: { clip_id: clip.id, rate, play } satisfies PlayReplayClipRequest,
    })
    playout.setStatus(clip.node_id, channelId, s)
    return s
  }

  return {
    buffers,
    clips,
    load,
    loadAll,
    codecsOf,
    nowUs,
    setBuffer,
    removeBuffer,
    play,
    loadClips,
    saveClip,
    renameClip,
    removeClip,
    exportClip,
    playClip,
  }
})
