import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { nodeApi } from '@/composables/useApi'
import { useNodesStore } from '@/stores/nodes'
import { blankLeg, presetLegs } from '@/stores/presets'
import type { PresetDto } from '@/types/generated/PresetDto'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'
import type { RecordingSessionDto } from '@/types/generated/RecordingSessionDto'
import type { StartRecordingRequest } from '@/types/generated/StartRecordingRequest'

/**
 * A node's session tagged with the node it lives on (`id` and `source_id` are
 * local to that node).
 */
export type RecordingSession = RecordingSessionDto & { node_id: string }

/** Video frames a session dropped across all its outputs. */
export function totalDropped(s: Pick<RecordingSessionDto, 'dropped_frames'>) {
  return s.dropped_frames.reduce((a, b) => a + b, 0)
}

export const useRecordingsStore = defineStore('recordings', () => {
  const nodes = useNodesStore()
  const sessions = ref<RecordingSession[]>([])

  const activeSessions = computed(() => sessions.value.filter((s) => s.status === 'active'))

  function upsert(nodeId: string, dto: RecordingSessionDto) {
    const session: RecordingSession = { ...dto, node_id: nodeId }
    const idx = sessions.value.findIndex((s) => s.node_id === nodeId && s.id === dto.id)
    if (idx === -1) sessions.value.push(session)
    else sessions.value[idx] = session
  }

  function find(nodeId: string, sessionId: string) {
    return sessions.value.find((s) => s.node_id === nodeId && s.id === sessionId)
  }

  function markStopped(nodeId: string, sessionId: string) {
    const session = find(nodeId, sessionId)
    if (session) session.status = 'stopped'
  }

  /** A leg failed but the session is still recording on its other legs. */
  function markLegFailed(nodeId: string, sessionId: string, error: string) {
    const session = find(nodeId, sessionId)
    if (session) session.error_message = error
  }

  /** Live per-output dropped-frame counts of an active session. */
  function setDropped(nodeId: string, sessionId: string, dropped: number[]) {
    const session = find(nodeId, sessionId)
    if (session) session.dropped_frames = dropped
  }

  function markError(nodeId: string, sessionId: string, error: string) {
    const session = find(nodeId, sessionId)
    if (session) {
      session.status = 'error'
      session.error_message = error
    }
  }

  async function loadForNode(nodeId: string) {
    const list = await nodeApi(nodeId)<RecordingSessionDto[]>('/recordings').catch(() => [])
    sessions.value = sessions.value.filter((s) => s.node_id !== nodeId)
    for (const dto of list) upsert(nodeId, dto)
  }

  async function load() {
    if (nodes.nodes.length === 0) await nodes.load()
    await Promise.all(nodes.reachable.map((n) => loadForNode(n.id)))
  }

  /**
   * Start recording `sourceId` on `nodeId`. The preset's outputs are sent
   * inline — nodes keep no preset store. With no preset, a single default
   * H.264/MOV output is used.
   */
  async function start(nodeId: string, sourceId: string, preset: PresetDto | null) {
    const outputs: PresetOutputInput[] = preset ? presetLegs(preset) : [blankLeg()]
    const body: StartRecordingRequest = {
      source_id: sourceId,
      preset_id: preset?.id ?? null,
      preset_name: preset?.name ?? null,
      outputs,
    }
    const dto = await nodeApi(nodeId)<RecordingSessionDto>('/recordings', { method: 'POST', body })
    upsert(nodeId, dto)
    return dto
  }

  async function stop(nodeId: string, sessionId: string) {
    const dto = await nodeApi(nodeId)<RecordingSessionDto>(`/recordings/${sessionId}/stop`, {
      method: 'POST',
    })
    upsert(nodeId, dto)
    return dto
  }

  function activeForSource(nodeId: string, sourceId: string): RecordingSession | null {
    return (
      activeSessions.value.find((s) => s.node_id === nodeId && s.source_id === sourceId) ?? null
    )
  }

  return {
    sessions,
    activeSessions,
    upsert,
    find,
    markStopped,
    markLegFailed,
    setDropped,
    markError,
    load,
    loadForNode,
    start,
    stop,
    activeForSource,
  }
})
