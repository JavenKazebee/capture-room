import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { nodeApi, sourceKey } from '@/composables/useApi'
import { useNodesStore } from '@/stores/nodes'
import { nodeLoadErrors } from '@/lib/loadState'
import { blankLeg, DEFAULT_PRESET, usePresetsStore } from '@/stores/presets'
import { useSourcesStore } from '@/stores/sources'
import type { ClockTimeDto } from '@/types/generated/ClockTimeDto'
import type { PresetDto } from '@/types/generated/PresetDto'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'
import type { RecordingSessionDto } from '@/types/generated/RecordingSessionDto'
import type { StartRecordingRequest } from '@/types/generated/StartRecordingRequest'

/** A session tagged with its node (`id` and `source_id` are node-local). */
export type RecordingSession = RecordingSessionDto & { node_id: string }

/** An output's files, or its planned path before it opens one (and for sessions with no file list). */
export function outputFiles(s: Pick<RecordingSessionDto, 'output_paths' | 'files' | 'status'>, i: number): string[] {
  if (!s.files.length) return [s.output_paths[i]!]
  const files = s.files[i] ?? []
  return files.length || s.status !== 'active' ? files : [s.output_paths[i]!]
}

/** The preset's preview output, else the first browser-playable one; `-1` if none. */
export function previewOutput(s: Pick<RecordingSessionDto, 'outputs'>): number {
  const marked = s.outputs.findIndex((o) => o.playable && o.preview)
  return marked !== -1 ? marked : s.outputs.findIndex((o) => o.playable)
}

export function totalDropped(s: Pick<RecordingSessionDto, 'dropped_frames'>) {
  return s.dropped_frames.reduce((a, b) => a + b, 0)
}

/** Sessions fetched from a node at a time. */
const PAGE = 100

export const useRecordingsStore = defineStore('recordings', () => {
  const nodes = useNodesStore()
  const sources = useSourcesStore()
  const presets = usePresetsStore()
  const sessions = ref<RecordingSession[]>([])
  /** Nodes whose history is fully loaded. */
  const complete = ref(new Set<string>())
  const loadingOlder = ref(false)
  const failed = nodeLoadErrors()
  /** Why the history may be incomplete or stale: the node list, or some nodes' sessions, failed to load. */
  const error = computed(() => nodes.error ?? failed.message('recordings', nodes.nameOf))

  const activeSessions = computed(() => sessions.value.filter((s) => s.status === 'active'))

  /** A node's oldest loaded session start, the cursor for its next page. */
  function oldestOf(nodeId: string): string | null {
    let oldest: string | null = null
    for (const s of sessions.value) {
      if (s.node_id === nodeId && (oldest === null || s.started_at < oldest)) oldest = s.started_at
    }
    return oldest
  }

  /**
   * History is paged per node, so nodes load different spans. Sessions before the
   * newest cursor among incomplete nodes may have gaps; `history` hides them.
   */
  const horizon = computed(() => {
    let h: string | null = null
    for (const n of nodes.reachable) {
      if (complete.value.has(n.id)) continue
      const oldest = oldestOf(n.id)
      if (oldest !== null && (h === null || oldest > h)) h = oldest
    }
    return h
  })

  /** Sessions newest first, without any that might have gaps before them. */
  const history = computed(() => {
    const h = horizon.value
    return sessions.value
      .filter((s) => h === null || s.started_at >= h)
      .sort((a, b) => b.started_at.localeCompare(a.started_at))
  })

  const hasOlder = computed(() => nodes.reachable.some((n) => !complete.value.has(n.id)))

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

  /** Live per-output dropped-frame counts and files of an active session. */
  function setStats(nodeId: string, sessionId: string, dropped: number[], files: string[][]) {
    const session = find(nodeId, sessionId)
    if (!session) return
    session.dropped_frames = dropped
    session.files = files
  }

  function markError(nodeId: string, sessionId: string, error: string) {
    const session = find(nodeId, sessionId)
    if (session) {
      session.status = 'error'
      session.error_message = error
    }
  }

  function setComplete(nodeId: string, done: boolean) {
    const next = new Set(complete.value)
    if (done) next.add(nodeId)
    else next.delete(nodeId)
    complete.value = next
  }

  /** A node's newest page (and its active sessions), replacing what was loaded. */
  async function loadForNode(nodeId: string) {
    const list = await failed.track(nodeId, () =>
      nodeApi(nodeId)<RecordingSessionDto[]>('/recordings', { query: { limit: PAGE } }),
    )
    if (!list) return
    // A short page is the whole history. A full one replaces only its span, so a
    // refresh mid-scroll keeps older pages.
    const whole = list.length < PAGE
    const oldest = list.reduce((min, s) => (s.started_at < min ? s.started_at : min), list[0]?.started_at ?? '')
    sessions.value = sessions.value.filter(
      (s) => s.node_id !== nodeId || (!whole && s.status !== 'active' && s.started_at < oldest),
    )
    for (const dto of list) upsert(nodeId, dto)
    if (whole) setComplete(nodeId, true)
  }

  /** The next page from every node with more history. */
  async function loadOlder() {
    if (loadingOlder.value) return
    loadingOlder.value = true
    try {
      await Promise.all(
        nodes.reachable
          .filter((n) => !complete.value.has(n.id))
          .map(async (n) => {
            const before = oldestOf(n.id)
            if (before === null) return setComplete(n.id, true)
            const list = await nodeApi(n.id)<RecordingSessionDto[]>('/recordings', {
              query: { before, limit: PAGE },
            })
            for (const dto of list) upsert(n.id, dto)
            setComplete(n.id, list.length < PAGE)
          }),
      )
    } finally {
      loadingOlder.value = false
    }
  }

  /** Remove a finished session. With `files`, deletes them first; any that can't be keep the session. */
  async function remove(nodeId: string, sessionId: string, { files = false } = {}) {
    const path = `/recordings/${encodeURIComponent(sessionId)}`
    try {
      await nodeApi(nodeId)(path, { method: 'DELETE', query: files ? { files: 1 } : undefined })
    } catch (e) {
      if (files) {
        const dto = await nodeApi(nodeId)<RecordingSessionDto>(path).catch(() => null)
        if (dto) upsert(nodeId, dto)
      }
      throw e
    }
    removeLocal(nodeId, sessionId)
  }

  /** Delete one file of a finished session; `path` guards against the list having changed. */
  async function deleteFile(nodeId: string, sessionId: string, output: number, file: number, path: string) {
    const dto = await nodeApi(nodeId)<RecordingSessionDto>(
      `/recordings/${encodeURIComponent(sessionId)}/outputs/${output}/files/${file}`,
      { method: 'DELETE', query: { path } },
    )
    upsert(nodeId, dto)
  }

  function removeLocal(nodeId: string, sessionId: string) {
    sessions.value = sessions.value.filter((s) => !(s.node_id === nodeId && s.id === sessionId))
  }

  /** The source's name when it recorded, else its current name. */
  function sourceNameOf(s: RecordingSession) {
    return (
      s.source_name ??
      sources.sources.find((x) => x.key === sourceKey(s.node_id, s.source_id))?.display_name ??
      s.source_id
    )
  }

  function presetNameOf(s: RecordingSession) {
    if (s.preset_name) return s.preset_name
    if (s.preset_id === DEFAULT_PRESET.id || !s.preset_id) return DEFAULT_PRESET.name
    return presets.presets.find((p) => p.id === s.preset_id)?.name ?? 'Unknown preset'
  }

  async function load() {
    if (nodes.nodes.length === 0) await nodes.load()
    failed.prune(nodes.reachable.map((n) => n.id))
    await Promise.all(nodes.reachable.map((n) => loadForNode(n.id)))
  }

  /**
   * The preset's outputs are sent inline (nodes keep no presets); none means one
   * H.264/MOV output. `startAt` is a time on the shared clock.
   */
  async function start(nodeId: string, sourceId: string, preset: PresetDto | null, startAt: ClockTimeDto | null = null) {
    const outputs: PresetOutputInput[] = preset ? preset.outputs : [blankLeg()]
    const body: StartRecordingRequest = {
      source_id: sourceId,
      preset_id: preset?.id ?? null,
      preset_name: preset?.name ?? null,
      outputs,
      start_at: startAt,
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
    error,
    activeSessions,
    history,
    hasOlder,
    loadingOlder,
    loadOlder,
    remove,
    deleteFile,
    removeLocal,
    sourceNameOf,
    presetNameOf,
    upsert,
    find,
    markStopped,
    markLegFailed,
    setStats,
    markError,
    load,
    loadForNode,
    start,
    stop,
    activeForSource,
  }
})
