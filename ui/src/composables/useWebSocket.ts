import { ref } from 'vue'
import { useSourcesStore, audioLevels, thumbnailSeqs } from '@/stores/sources'
import { totalDropped, useRecordingsStore } from '@/stores/recordings'
import { useNodesStore } from '@/stores/nodes'
import { sourceKey } from '@/composables/useApi'
import { useEventsStore } from '@/stores/events'
import { useCapacityStore } from '@/stores/capacity'
import { usePlayoutStore } from '@/stores/playout'
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
    if (connectedBefore) useEventsStore().log('info', 'Reconnected to server')
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
    if (wsStatus.value === 'connected') useEventsStore().log('warn', 'Lost connection to server — reconnecting')
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
  logEvent(event)

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
      sources.updateStatus(nodeId, event.source_id, event.timecode, event.error, event.link)
      break

    case 'audio.levels':
      audioLevels.set(sourceKey(nodeId, event.source_id), event.channels)
      break

    case 'thumbnail.updated': {
      const key = sourceKey(nodeId, event.source_id)
      thumbnailSeqs.set(key, (thumbnailSeqs.get(key) ?? 0) + 1)
      break
    }

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
  }
}

export function startWebSocket() {
  setTimeout(connect, 0)
}

/** Record the notable events in the event log, before the stores apply them. */
function logEvent(event: NodeEvent) {
  const log = useEventsStore().log
  const node_id = event.node_id
  const nodes = useNodesStore()
  const source = (id: string) =>
    useSourcesStore().sources.find((s) => s.key === sourceKey(node_id, id))?.display_name ?? id

  switch (event.type) {
    case 'node.online':
      log('info', `Node online: ${nodes.nameOf(event.peer_id)}`, { node_id: event.peer_id })
      break
    case 'node.offline':
      log('warn', `Node offline: ${nodes.nameOf(event.peer_id)}`, { node_id: event.peer_id })
      break
    case 'recording.started':
      log('rec', `Recording started: ${source(event.source_id)}`, { node_id, detail: event.session_id })
      break
    case 'recording.stopped':
      log('info', `Recording stopped: ${source(event.source_id)}`, { node_id, detail: event.session_id })
      break
    case 'recording.leg_failed':
      log('error', `Output failed on ${source(event.source_id)} — still recording other outputs`, {
        node_id,
        detail: event.error,
      })
      break
    case 'recording.error':
      log('error', `Recording failed: ${source(event.source_id)}`, { node_id, detail: event.error })
      break
    case 'recording.stats': {
      // Only the first drop of a session: the stats repeat every second.
      const prev = useRecordingsStore().find(node_id, event.session_id)
      const now = event.dropped_frames.reduce((a, b) => a + b, 0)
      if (prev && totalDropped(prev) === 0 && now > 0) {
        log('warn', `Dropping frames on ${source(event.source_id)} — an encoder can't keep up`, {
          node_id,
          detail: event.session_id,
        })
      }
      break
    }
    case 'benchmark.updated': {
      // Only the start and the end; steps in between update the node card.
      const run = event.run
      const name = run.preset_name ?? 'H.264 (default)'
      if (run.status === 'running') {
        if (run.feeds_running === 0) log('info', `Benchmark started: ${name}`, { node_id })
      } else if (run.status === 'completed') {
        const n = run.sustainable_feeds
        const atLimit = n >= run.max_feeds
        log('info', `Benchmark finished: ${name} sustains ${atLimit ? 'at least ' : ''}${n} feed${n === 1 ? '' : 's'}`, {
          node_id,
          detail: run.message ?? undefined,
        })
      } else {
        log(run.status === 'error' ? 'error' : 'warn', `Benchmark ${run.status === 'error' ? 'failed' : 'cancelled'}: ${name}`, {
          node_id,
          detail: run.message ?? undefined,
        })
      }
      break
    }
    case 'channel.state': {
      // Only transitions: a clip that failed, an output that stopped.
      const prev = usePlayoutStore().status.get(sourceKey(node_id, event.source_id))
      const s = event.status
      if (s.error && s.error !== prev?.error) {
        log('error', `Playout failed on ${source(event.source_id)}`, { node_id, detail: s.error })
      }
      for (const o of s.outputs) {
        const was = prev?.outputs.find((p) => p.label === o.label)
        if (o.error && o.error !== was?.error) {
          log('error', `Output failed: ${o.label} on ${source(event.source_id)}`, { node_id, detail: o.error })
        }
      }
      break
    }
    case 'feed.status': {
      // Only transitions: the status event repeats every tick.
      const prev = useSourcesStore().sources.find((s) => s.key === sourceKey(node_id, event.source_id))
      if (!prev) break
      if (prev.error !== event.error) {
        if (event.error) log('error', `Source failed: ${prev.display_name}`, { node_id, detail: event.error })
        else log('info', `Source recovered: ${prev.display_name}`, { node_id })
      }
      // Live sources: lost and regained signal (not the first connect).
      if (prev.link && event.link && prev.link !== event.link) {
        if (prev.link === 'live' && event.link === 'reconnecting') {
          log('warn', `Signal lost: ${prev.display_name} — reconnecting, recording black`, { node_id })
        } else if (prev.link === 'live' && event.link === 'waiting') {
          log('info', `Sender disconnected: ${prev.display_name}`, { node_id })
        } else if (event.link === 'live') {
          log('info', `Signal: ${prev.display_name} is live`, { node_id })
        }
      }
      break
    }
  }
}
