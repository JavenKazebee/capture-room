import { ref } from 'vue'
import { useSourcesStore, audioLevels, thumbnailSeqs } from '@/stores/sources'
import { useRecordingsStore } from '@/stores/recordings'
import { useNodesStore } from '@/stores/nodes'
import { sourceKey } from '@/composables/useApi'

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

// Every event carries the `node_id` it describes; source and session ids
// inside it are local to that node.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function handleEvent(event: Record<string, any>) {
  const sources = useSourcesStore()
  const recordings = useRecordingsStore()
  const nodes = useNodesStore()
  const nodeId = event.node_id as string

  switch (event.type) {
    case 'node.online':
    case 'node.offline':
      nodes.load().then(() => Promise.all([sources.loadSources(), recordings.load()]))
      break

    case 'recording.started':
      // Usually already in the store via the POST response; reload in case it
      // came from another client or a schedule.
      if (!recordings.activeForSource(nodeId, event.source_id as string)) {
        recordings.loadForNode(nodeId)
      }
      break

    case 'recording.stopped':
      recordings.markStopped(nodeId, event.session_id as string)
      break

    case 'recording.error':
      recordings.markError(nodeId, event.session_id as string, event.error as string)
      break

    case 'feed.status':
      sources.updateTimecode(nodeId, event.source_id as string, event.timecode as string | null)
      break

    case 'audio.levels': {
      const channels = event.channels as { peak_db: number; rms_db: number }[]
      audioLevels.set(sourceKey(nodeId, event.source_id as string), channels)
      break
    }

    case 'thumbnail.updated': {
      const key = sourceKey(nodeId, event.source_id as string)
      thumbnailSeqs.set(key, (thumbnailSeqs.get(key) ?? 0) + 1)
      break
    }
  }
}

export function startWebSocket() {
  setTimeout(connect, 0)
}
