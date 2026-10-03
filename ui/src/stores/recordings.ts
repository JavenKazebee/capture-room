import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
<<<<<<< HEAD
import { nodeApi } from '@/composables/useApi'
import { useNodesStore } from '@/stores/nodes'
import { blankLeg, type OutputLegInput, type Preset } from '@/stores/presets'

/** As returned by a node: `source_id` is local to that node. */
export interface RecordingSessionDto {
  id: string
  source_id: string
  preset_id: string
  started_at: string
  stopped_at: string | null
  output_paths: string[]
  status: 'active' | 'stopped' | 'error'
  error_message: string | null
}

export interface RecordingSession extends RecordingSessionDto {
  node_id: string
=======
import { useApi } from '@/composables/useApi'
import type { RecordingSessionDto } from '@/types/generated/RecordingSessionDto'

// status is a plain string on the wire; narrow it to the values the node emits.
// node_id is added by the aggregator when proxying a remote node's sessions.
export type RecordingSession = Omit<RecordingSessionDto, 'status'> & {
  status: 'active' | 'stopped' | 'error'
  node_id?: string
>>>>>>> claude/lucid-bun-9f1bf1
}

export const useRecordingsStore = defineStore('recordings', () => {
  const nodes = useNodesStore()
  const sessions = ref<RecordingSession[]>([])

  const activeSessions = computed(() => sessions.value.filter((s) => s.status === 'active'))

  function upsert(nodeId: string, dto: RecordingSessionDto) {
    const session = { ...dto, node_id: nodeId }
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
  async function start(nodeId: string, sourceId: string, preset: Preset | null) {
    const outputs: OutputLegInput[] = preset
      ? preset.outputs.map(({ id: _id, preset_id: _p, sort_order: _s, ...leg }) => leg)
      : [blankLeg()]
    const dto = await nodeApi(nodeId)<RecordingSessionDto>('/recordings', {
      method: 'POST',
      body: { source_id: sourceId, preset_id: preset?.id ?? null, outputs },
    })
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
    markStopped,
    markError,
    load,
    loadForNode,
    start,
    stop,
    activeForSource,
  }
})
