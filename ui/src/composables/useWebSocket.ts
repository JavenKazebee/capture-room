import { ref } from 'vue'
import { useSourcesStore, audioLevels, thumbnailSeqs } from '@/stores/sources'
import { useRecordingsStore } from '@/stores/recordings'

export type WsStatus = 'connecting' | 'connected' | 'disconnected'

const BASE_DELAY = 1_000
const MAX_DELAY = 16_000

let socket: WebSocket | null = null
let reconnectTimer: ReturnType<typeof setTimeout> | null = null
let attempt = 0

export const wsStatus = ref<WsStatus>('disconnected')

function connect() {
  if (socket && socket.readyState <= WebSocket.OPEN) return

  const proto = location.protocol === 'https:' ? 'wss' : 'ws'
  socket = new WebSocket(`${proto}://${location.host}/ws`)
  wsStatus.value = 'connecting'

  socket.addEventListener('open', () => {
    wsStatus.value = 'connected'
    attempt = 0
    if (reconnectTimer) {
      clearTimeout(reconnectTimer)
      reconnectTimer = null
    }
  })

  socket.addEventListener('message', (ev) => {
    try {
      handleEvent(JSON.parse(ev.data as string))
    } catch {
      // ignore malformed frames
    }
  })

  socket.addEventListener('close', () => {
    wsStatus.value = 'disconnected'
    const delay = Math.min(BASE_DELAY * 2 ** attempt, MAX_DELAY)
    attempt++
    reconnectTimer = setTimeout(connect, delay)
  })

  socket.addEventListener('error', () => {
    socket?.close()
  })
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function handleEvent(event: Record<string, any>) {
  const sources = useSourcesStore()
  const recordings = useRecordingsStore()

  switch (event.type) {
    case 'source.available':
    case 'node.online':
    case 'node.offline':
      sources.loadSources()
      break

    case 'source.lost':
      sources.remove(event.source_id as string)
      break

    case 'recording.started':
      // The session was already added to the store via the POST response.
      // If it arrived from a different client, reload recordings.
      break

    case 'recording.stopped':
      recordings.markStopped(event.session_id as string)
      break

    case 'recording.error':
      recordings.markError(event.session_id as string, event.error as string)
      break

    case 'feed.status': {
      sources.updateTimecode(event.source_id as string, event.timecode as string | null)
      break
    }

    case 'audio.levels': {
      const sourceId = event.source_id as string
      const channels = event.channels as { peak_db: number; rms_db: number }[]
      audioLevels.set(sourceId, channels)
      break
    }

    case 'thumbnail.updated': {
      const sourceId = event.source_id as string
      thumbnailSeqs.set(sourceId, (thumbnailSeqs.get(sourceId) ?? 0) + 1)
      break
    }
  }
}

export function useWebSocket() {
  return { status: wsStatus }
}

export function startWebSocket() {
  setTimeout(connect, 0)
}
