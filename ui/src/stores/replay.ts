import { defineStore } from 'pinia'
import { reactive } from 'vue'
import { nodeApi, sourceKey } from '@/composables/useApi'
import { onReconnect } from '@/lib/reconnect'
import { nodeNowUs, replaySpan } from '@/lib/replaySpan'
import { useNodesStore } from '@/stores/nodes'
import { usePlayoutStore } from '@/stores/playout'
import type { ChannelStatusDto } from '@/types/generated/ChannelStatusDto'
import type { ReplayBufferDto } from '@/types/generated/ReplayBufferDto'
import type { ReplayClipRequest } from '@/types/generated/ReplayClipRequest'
import type { ReplayCodec } from '@/types/generated/ReplayCodec'
import type { ReplayConfigInput } from '@/types/generated/ReplayConfigInput'
import type { ReplayClipDto } from '@/types/generated/ReplayClipDto'
import type { SaveReplayClipRequest } from '@/types/generated/SaveReplayClipRequest'
import type { SaveReplayClipGroupRequest } from '@/types/generated/SaveReplayClipGroupRequest'
import type { ReplayClipGroupDto } from '@/types/generated/ReplayClipGroupDto'
import type { MediaItemDto } from '@/types/generated/MediaItemDto'
import type { PlayReplayClipRequest } from '@/types/generated/PlayReplayClipRequest'
import type { UpdateReplayClipRequest } from '@/types/generated/UpdateReplayClipRequest'

export type ReplayBuffer = ReplayBufferDto & {
  node_id: string
  key: string
  /** `performance.now()` when it was reported, to run its clock and span on from `now_us`. */
  fetched_at: number
}

export type ReplayClip = ReplayClipDto & {
  node_id: string
  /** `node_id/id`. */
  key: string
}

/** Clips saved together on several angles (Shift+S), each named after its angle. */
export type ReplayClipGroup = {
  id: string
  node_id: string
  /** `node_id/id`. */
  key: string
  name: string
  clips: ReplayClip[]
}

/**
 * Replay buffers on every node and their commands; channel status lives in the playout store. Nodes
 * send `replay.buffers` only on a change: in between, `span` runs each buffer on.
 */
export const useReplayStore = defineStore('replay', () => {
  const nodes = useNodesStore()
  const playout = usePlayoutStore()
  /** By source key. */
  const buffers = reactive(new Map<string, ReplayBuffer>())
  /** Saved clips, by node, newest first. */
  const clips = reactive(new Map<string, ReplayClip[]>())

  /** Nodes whose buffers have been loaded, to load again on reconnect. */
  const loaded = new Set<string>()

  /** A node's whole list, from `/replay` or an event. Entries are patched, so only what changed re-renders. */
  function apply(nodeId: string, list: ReplayBufferDto[]) {
    const now = performance.now()
    loaded.add(nodeId)
    const keys = new Set<string>()
    for (const b of list) {
      const key = sourceKey(nodeId, b.source_id)
      keys.add(key)
      const entry = { ...b, node_id: nodeId, key, fetched_at: now }
      const had = buffers.get(key)
      if (had) Object.assign(had, entry)
      else buffers.set(key, entry)
    }
    for (const [key, b] of buffers) if (b.node_id === nodeId && !keys.has(key)) buffers.delete(key)
  }

  async function load(nodeId: string) {
    apply(nodeId, await nodeApi(nodeId)<ReplayBufferDto[]>('/replay'))
  }

  async function loadAll() {
    if (!nodes.nodes.length) await nodes.load()
    await Promise.all(nodes.reachable.map((n) => load(n.id).catch(() => {})))
  }

  onReconnect(() => Promise.all([...loaded].map((id) => load(id).catch(() => {}))))

  /** The codecs a node can keep buffers in, default first; empty if it isn't listed. */
  function codecsOf(nodeId: string): ReplayCodec[] {
    return nodes.nodes.find((n) => n.id === nodeId)?.replay_codecs ?? []
  }

  /** A buffer's node clock now (µs), run on from when it was reported. */
  function nowUs(b: ReplayBuffer, at = performance.now()) {
    return nodeNowUs(b, at)
  }

  /** What a buffer can replay now, run on from when it was reported; `null` while empty. */
  function span(b: ReplayBuffer, at = performance.now()) {
    return replaySpan(b, at)
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

  /** Save in → out on several angles as one group. */
  async function saveClipGroup(nodeId: string, req: SaveReplayClipGroupRequest) {
    const g = await nodeApi(nodeId)<ReplayClipGroupDto>('/replay/clip-groups', { method: 'POST', body: req })
    await loadClips(nodeId)
    return g
  }

  async function renameClipGroup(group: ReplayClipGroup, name: string) {
    await nodeApi(group.node_id)(`/replay/clip-groups/${group.id}`, {
      method: 'PATCH',
      body: { name } satisfies UpdateReplayClipRequest,
    })
    await loadClips(group.node_id)
  }

  /** Delete every angle's clip in a group, and their files. */
  async function removeClipGroup(group: ReplayClipGroup) {
    await nodeApi(group.node_id)(`/replay/clip-groups/${group.id}`, { method: 'DELETE' })
    await loadClips(group.node_id)
  }

  /** Export every angle's clip in a group, a file each. */
  async function exportClipGroup(group: ReplayClipGroup) {
    return nodeApi(group.node_id)<MediaItemDto[]>(`/replay/clip-groups/${group.id}/export`, { method: 'POST' })
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

  /** Put a saved clip on a channel (same node), from its first frame or from `atUs` (node clock). */
  async function playClip(channelId: string, clip: ReplayClip, rate: number, play: boolean, atUs: number | null = null) {
    const s = await nodeApi(clip.node_id)<ChannelStatusDto>(`/channels/${channelId}/replay-clip`, {
      method: 'POST',
      body: { clip_id: clip.id, rate, play, at_us: atUs === null ? null : Math.round(atUs) } satisfies PlayReplayClipRequest,
    })
    playout.setStatus(clip.node_id, channelId, s)
    return s
  }

  return {
    buffers,
    clips,
    apply,
    load,
    loadAll,
    codecsOf,
    nowUs,
    span,
    setBuffer,
    removeBuffer,
    play,
    loadClips,
    saveClip,
    saveClipGroup,
    renameClipGroup,
    removeClipGroup,
    exportClipGroup,
    renameClip,
    removeClip,
    exportClip,
    playClip,
  }
})
