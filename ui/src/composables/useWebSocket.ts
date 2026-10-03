import { ref } from 'vue'
import { useSourcesStore, audioLevels, thumbnailSeqs } from '@/stores/sources'
import { useRecordingsStore } from '@/stores/recordings'
import { useNodesStore } from '@/stores/nodes'
import { sourceKey } from '@/composables/useApi'
import type { WsEvent } from '@/types/generated/WsEvent'

export type WsStatus = 'connecting' | 'connected' | 'disconnected'

const BASE_DELAY = 1_000
const MAX_DELAY = 16_000

let socket: WebSocket | null = null
let reconnectTimer: ReturnType<typeof setTimeout> | null = null
let attempt = 0
/** Set once the first connection opens; any later open is a reconnect. */
let connectedBefore = false

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
    // Events sent while disconnected are lost (a recording that stopped or
    // failed, a node that went away), so reload rather than show stale state.
    if (connectedBefore) reloadAll()
    connectedBefore = true
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

/** Reload nodes, then every reachable node's sources and recordings. */
export async function reloadAll() {
  const nodes = useNodesStore()
  await nodes.load()
  await Promise.all([useSourcesStore().loadSources(), useRecordingsStore().load()])
}

// Every event carries the `node_id` it describes; source and session ids
// inside it are local to that node.
type NodeEvent = WsEvent & { node_id: string }

function handleEvent(event: NodeEvent) {
  const sources = useSourcesStore()
  const recordings = useRecordingsStore()
  const nodeId = event.node_id

  switch (event.type) {
    case 'node.online':
    case 'node.offline':
      reloadAll()
      break

    case 'node.updated':
      useNodesStore().load()
      break

    case 'recording.started':
      // Usually already in the store via the POST response; reload in case it
      // came from another client or a schedule.
      if (!recordings.activeForSource(nodeId, event.source_id)) {
        recordings.loadForNode(nodeId)
      }
      break

    case 'recording.stopped':
      recordings.markStopped(nodeId, event.session_id)
      break

    case 'recording.leg_failed':
      recordings.markLegFailed(nodeId, event.session_id, event.error)
      break

    case 'recording.error':
      recordings.markError(nodeId, event.session_id, event.error)
      break

    case 'feed.status':
      sources.updateStatus(nodeId, event.source_id, event.timecode, event.error)
      break

    case 'audio.levels':
      audioLevels.set(sourceKey(nodeId, event.source_id), event.channels)
      break

    case 'thumbnail.updated': {
      const key = sourceKey(nodeId, event.source_id)
      thumbnailSeqs.set(key, (thumbnailSeqs.get(key) ?? 0) + 1)
      break
    }
  }
}

export function startWebSocket() {
  setTimeout(connect, 0)
}
