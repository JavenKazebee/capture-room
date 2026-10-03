import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { useApi } from '@/composables/useApi'
import type { RecordingSessionDto } from '@/types/generated/RecordingSessionDto'

// status is a plain string on the wire; narrow it to the values the node emits.
// node_id is added by the aggregator when proxying a remote node's sessions.
export type RecordingSession = Omit<RecordingSessionDto, 'status'> & {
  status: 'active' | 'stopped' | 'error'
  node_id?: string
}

export const useRecordingsStore = defineStore('recordings', () => {
  const sessions = ref<RecordingSession[]>([])

  const activeSessions = computed(() => sessions.value.filter((s) => s.status === 'active'))

  function upsert(session: RecordingSession) {
    const idx = sessions.value.findIndex((s) => s.id === session.id)
    if (idx === -1) sessions.value.push(session)
    else sessions.value[idx] = session
  }

  function markStopped(sessionId: string) {
    const session = sessions.value.find((s) => s.id === sessionId)
    if (session) session.status = 'stopped'
  }

  function markError(sessionId: string, error: string) {
    const session = sessions.value.find((s) => s.id === sessionId)
    if (session) {
      session.status = 'error'
      session.error_message = error
    }
  }

  async function start(sourceId: string, presetId: string): Promise<RecordingSession> {
    const { api } = useApi()
    const session = await api<RecordingSession>('/recordings', {
      method: 'POST',
      body: { source_id: sourceId, preset_id: presetId },
    })
    upsert(session)
    return session
  }

  async function stop(sessionId: string): Promise<RecordingSession> {
    const { api } = useApi()
    const session = await api<RecordingSession>(`/recordings/${sessionId}`, {
      method: 'PATCH',
      body: { action: 'stop' },
    })
    upsert(session)
    return session
  }

  function activeForSource(sourceId: string): RecordingSession | null {
    return activeSessions.value.find((s) => s.source_id === sourceId) ?? null
  }

  return { sessions, activeSessions, upsert, markStopped, markError, start, stop, activeForSource }
})
