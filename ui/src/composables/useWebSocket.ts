import { onUnmounted, ref } from 'vue'
import { useNodesStore } from '@/stores/nodes'
import { useRecordingsStore } from '@/stores/recordings'
import { useSourcesStore } from '@/stores/sources'

type WsStatus = 'connecting' | 'connected' | 'disconnected'

const RECONNECT_DELAY_MS = 3000

let socket: WebSocket | null = null
let reconnectTimer: ReturnType<typeof setTimeout> | null = null
const status = ref<WsStatus>('disconnected')
const subscribers = new Set<() => void>()

function connect() {
  if (socket && socket.readyState <= WebSocket.OPEN) return

  const proto = location.protocol === 'https:' ? 'wss' : 'ws'
  socket = new WebSocket(`${proto}://${location.host}/ws`)
  status.value = 'connecting'

  socket.addEventListener('open', () => {
    status.value = 'connected'
    if (reconnectTimer) clearTimeout(reconnectTimer)
  })

  socket.addEventListener('message', (ev) => {
    try {
      handleEvent(JSON.parse(ev.data))
    } catch {
      // ignore malformed frames
    }
  })

  socket.addEventListener('close', () => {
    status.value = 'disconnected'
    reconnectTimer = setTimeout(connect, RECONNECT_DELAY_MS)
  })

  socket.addEventListener('error', () => {
    socket?.close()
  })
}

function handleEvent(event: Record<string, unknown>) {
  const nodes = useNodesStore()
  const sources = useSourcesStore()
  const recordings = useRecordingsStore()

  switch (event.type) {
    case 'node.online':
      nodes.setStatus(event.node_id as string, 'online')
      break
    case 'node.offline':
      nodes.setStatus(event.node_id as string, 'offline')
      break
    case 'source.available':
      sources.upsert({
        id: event.source_id as string,
        node_id: event.node_id as string,
        display_name: event.name as string,
        source_type: event.source_type as string,
        is_available: true,
        timecode: null,
      })
      break
    case 'source.lost':
      sources.remove(event.node_id as string, event.source_id as string)
      break
    case 'recording.started':
      // full session object will come from the REST response; WS just signals the state change
      break
    case 'recording.stopped':
      recordings.stop(event.session_id as string, event.stopped_at as string)
      break
    case 'feed.status':
      sources.upsert({
        id: event.source_id as string,
        node_id: event.node_id as string,
        display_name: event.display_name as string,
        source_type: event.source_type as string,
        is_available: true,
        timecode: (event.timecode as string) ?? null,
      })
      break
  }
}

export function useWebSocket() {
  onUnmounted(() => {
    subscribers.delete(connect)
  })

  return { status, connect }
}

export function startWebSocket() {
  connect()
}
