import { ref } from 'vue'
import { useSourcesStore, audioLevels } from '@/stores/sources'
import { useRecordingsStore } from '@/stores/recordings'
import { useNodesStore } from '@/stores/nodes'
import { sourceKey } from '@/composables/useApi'
import { useEventsStore } from '@/stores/events'
import { useCapacityStore } from '@/stores/capacity'
import { usePlayoutStore } from '@/stores/playout'
import { useReplayStore } from '@/stores/replay'
import { useSchedulesStore } from '@/stores/schedules'
import { runReconnectReloads } from '@/lib/reconnect'
import { checkAuth, needsSignIn } from '@/composables/useAuth'
import { reconnectingSocket } from '@/lib/reconnectingSocket'
import { logEvent } from '@/lib/eventLog'
import type { NodeEvent } from '@/types/generated/NodeEvent'

export type WsStatus = 'connecting' | 'connected' | 'disconnected'

export const wsStatus = ref<WsStatus>('disconnected')
/** Failed connection attempts since the socket was last open; 0 while connected. */
export const reconnectAttempt = ref(0)
/** Whether the socket has opened at least once since the page loaded. */
export const wsEverConnected = ref(false)

const socket = reconnectingSocket({
  path: '/ws',
  // Refused until signed in; signing in reloads the page.
  shouldReconnect: () => !needsSignIn.value,
  onConnecting: () => (wsStatus.value = 'connecting'),
  onOpen(reconnect) {
    wsStatus.value = 'connected'
    reconnectAttempt.value = 0
    wsEverConnected.value = true
    if (reconnect) {
      useEventsStore().log('info', 'Reconnected to server')
      // Events sent while disconnected are lost (a recording that stopped or
      // failed, a node that went away), so reload rather than show stale state.
      reloadAfterReconnect()
    }
  },
  onMessage: (data) => handleEvent(data as NodeEvent),
  onClose(wasOpen) {
    if (wasOpen) useEventsStore().log('warn', 'Lost connection to server — reconnecting')
    wsStatus.value = 'disconnected'
    reconnectAttempt.value = socket.attempt
    // A changed token closes nothing else the page would notice.
    checkAuth()
  },
})

/** Reload after missed events: nodes, sources and recordings, then every `onReconnect` handler. */
async function reloadAfterReconnect() {
  await reloadAll()
  await runReconnectReloads()
}

/** Connect now instead of waiting out the backoff (the tab came back, the network did). */
export function reconnectNow() {
  socket.reconnectNow()
}

/** Reload nodes, then every reachable node's sources and recordings. */
export async function reloadAll() {
  const nodes = useNodesStore()
  await nodes.load()
  await Promise.all([useSourcesStore().loadSources(), useRecordingsStore().load()])
}

function handleEvent(event: NodeEvent) {
  const sources = useSourcesStore()
  const recordings = useRecordingsStore()
  const nodeId = event.node_id
  logEvent(event)

  switch (event.type) {
    case 'node.online':
    case 'node.offline':
    case 'node.removed':
      reloadAll()
      break

    case 'node.updated':
      useNodesStore().load()
      break

    case 'sources.changed':
      sources.refreshNode(nodeId).catch(() => {})
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

    case 'recording.removed':
      recordings.removeLocal(nodeId, event.session_id)
      break

    case 'recording.updated':
      recordings.upsert(nodeId, event.session)
      break

    case 'recording.stats':
      recordings.setStats(nodeId, event.session_id, event.dropped_frames, event.files)
      break

    case 'feed.status':
      sources.updateStatus(nodeId, event.source_id, event)
      break

    case 'audio.levels':
      audioLevels.set(sourceKey(nodeId, event.source_id), event.channels)
      break

    case 'benchmark.updated':
      useCapacityStore().applyRun(nodeId, event.run)
      break

    case 'channel.state':
      usePlayoutStore().setStatus(nodeId, event.source_id, event.status)
      break

    case 'media.updated': {
      const playout = usePlayoutStore()
      if (playout.media.has(nodeId)) playout.loadMedia(nodeId)
      break
    }

    case 'replay.clips': {
      const replay = useReplayStore()
      if (replay.clips.has(nodeId)) replay.loadClips(nodeId)
      break
    }

    case 'replay.buffers':
      useReplayStore().apply(nodeId, event.buffers)
      break

    case 'schedule.updated':
      useSchedulesStore().refresh(event.schedule_id)
      break
  }
}

export function startWebSocket() {
  setTimeout(socket.connect, 0)
}

// A hot update would leave the old socket running beside a module that never connects.
if (import.meta.hot) {
  import.meta.hot.accept(() => window.location.reload())
}
