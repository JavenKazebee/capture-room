import { defineStore } from 'pinia'
import { ref } from 'vue'

export interface RecordingSession {
  id: string
  node_id: string
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

  const active = () => sessions.value.filter((s) => s.status === 'active')

  function upsert(session: RecordingSession) {
    const idx = sessions.value.findIndex((s) => s.id === session.id)
    if (idx === -1) sessions.value.push(session)
    else sessions.value[idx] = session
  }

  function stop(id: string, stoppedAt: string) {
    const session = sessions.value.find((s) => s.id === id)
    if (session) {
      session.status = 'stopped'
      session.stopped_at = stoppedAt
    }
  }

  return { sessions, active, upsert, stop }
})
