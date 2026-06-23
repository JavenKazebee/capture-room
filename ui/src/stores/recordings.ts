import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { useApi } from '@/composables/useApi'

export interface RecordingSession {
  id: string
  source_id: string
  preset_id: string
  started_at: string
  stopped_at: string | null
  primary_path: string
  secondary_path: string | null
  redundant_path: string | null
  status: 'active' | 'stopped' | 'error'
  error_message: string | null
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
