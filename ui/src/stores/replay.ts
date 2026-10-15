import { defineStore } from 'pinia'
import { reactive } from 'vue'
import { nodeApi, sourceKey } from '@/composables/useApi'
import { useNodesStore } from '@/stores/nodes'
import { usePlayoutStore } from '@/stores/playout'
import type { ChannelStatusDto } from '@/types/generated/ChannelStatusDto'
import type { NodeStatus } from '@/types/generated/NodeStatus'
import type { ReplayBufferDto } from '@/types/generated/ReplayBufferDto'
import type { ReplayClipRequest } from '@/types/generated/ReplayClipRequest'
import type { ReplayCodec } from '@/types/generated/ReplayCodec'
import type { ReplayConfigInput } from '@/types/generated/ReplayConfigInput'

export type ReplayBuffer = ReplayBufferDto & {
  node_id: string
  /** The source's key. */
  key: string
  /** `performance.now()` when it was fetched, to run its clock on from `now_us`. */
  fetched_at: number
}

export const CODECS: Record<ReplayCodec, { label: string; note: string }> = {
  prores_proxy: { label: 'ProRes 422 Proxy', note: 'About 40 GB an hour at 1080p59.94. Mac only.' },
  h264_intra: { label: 'H.264, every frame a keyframe', note: 'A fraction of the size.' },
  mjpeg: { label: 'Motion JPEG', note: 'About 40 GB an hour at 1080p59.94. Any platform.' },
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
  /** The codecs each node can use, default first. */
  const codecs = reactive(new Map<string, ReplayCodec[]>())

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

  async function loadCodecs(nodeId: string) {
    if (codecs.has(nodeId)) return codecs.get(nodeId)!
    const status = await nodeApi(nodeId)<NodeStatus>('/status')
    codecs.set(nodeId, status.replay_codecs ?? [])
    return codecs.get(nodeId)!
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

  async function speed(nodeId: string, channelId: string, rate: number) {
    const s = await nodeApi(nodeId)<ChannelStatusDto>(`/channels/${channelId}/transport`, {
      method: 'POST',
      body: { action: 'speed', rate, position_ms: null, frames: null },
    })
    playout.setStatus(nodeId, channelId, s)
  }

  async function step(nodeId: string, channelId: string, frames: number) {
    const s = await nodeApi(nodeId)<ChannelStatusDto>(`/channels/${channelId}/transport`, {
      method: 'POST',
      body: { action: 'step', frames, position_ms: null, rate: null },
    })
    playout.setStatus(nodeId, channelId, s)
  }

  return { buffers, codecs, load, loadAll, loadCodecs, nowUs, setBuffer, removeBuffer, play, speed, step }
})
