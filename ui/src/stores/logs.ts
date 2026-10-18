import { defineStore, storeToRefs } from 'pinia'
import { computed, reactive, ref, shallowRef, triggerRef, watch } from 'vue'
import { nodeApi } from '@/composables/useApi'
import { needsSignIn } from '@/composables/useAuth'
import { useEventsStore, type LogEntry } from '@/stores/events'
import { useNodesStore } from '@/stores/nodes'
import type { LogLevel } from '@/types/generated/LogLevel'
import type { LogLine } from '@/types/generated/LogLine'
import type { LogPage } from '@/types/generated/LogPage'
import type { LogSubscription } from '@/types/generated/LogSubscription'

export type RowLevel = LogLevel | 'rec'
export type RowSource = 'node' | 'activity'

/** One line of the Logs view: a node's log line or an activity event. */
export interface LogRow {
  key: string
  at: Date
  level: RowLevel
  node_id: string | null
  message: string
  /** A node line's fields, an event's detail. */
  detail: string
  source: RowSource
  /** Node lines only. */
  target?: string
  seq?: number
}

/** Rows kept in the view; older ones drop off the top. */
const MAX_ROWS = 20_000
/** Lines fetched per node when the view opens, and per "Load older". */
const PAGE = 1000
const BASE_DELAY = 500
const MAX_DELAY = 3_000

function fromLine(l: LogLine, nodeId: string): LogRow {
  return {
    // Stand-in lines (seq 0) are never repeated by a fetch, so any unique key does.
    key: l.seq ? `${nodeId}/${l.seq}` : `${nodeId}/x${Math.random()}`,
    at: new Date(l.at),
    level: l.level,
    node_id: nodeId,
    message: l.message,
    detail: l.fields,
    source: 'node',
    target: l.target,
    seq: l.seq,
  }
}

function fromEntry(e: LogEntry): LogRow {
  return {
    key: `a/${e.id}`,
    at: e.at,
    level: e.level,
    node_id: e.node_id,
    message: e.message,
    detail: e.detail ?? '',
    source: 'activity',
  }
}

/**
 * Every reachable node's log lines (fetched, then streamed over `/ws/logs`)
 * merged with the page's activity events. Live only while a view has it open.
 */
export const useLogsStore = defineStore('logs', () => {
  const events = useEventsStore()
  const nodes = useNodesStore()

  // A shallow ref over a mutated array, like the events store.
  const rows = shallowRef<LogRow[]>([])
  const keys = new Set<string>()
  /** Lines at this level and more severe are fetched and streamed. */
  const level = ref<LogLevel>('info')
  /** Each node's log file, when it runs as the service. */
  const logFiles = reactive(new Map<string, string | null>())
  /** Nodes with nothing older left in memory. */
  const exhausted = reactive(new Set<string>())
  const loadingOlder = ref(false)

  let users = 0
  let socket: WebSocket | null = null
  let attempt = 0
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null
  let lastEventId = 0
  let stopWatches: (() => void)[] = []

  const nodeIds = computed(() => nodes.reachable.map((n) => n.id))

  function add(newRows: LogRow[], sort: boolean) {
    let added = false
    for (const r of newRows) {
      if (keys.has(r.key)) continue
      keys.add(r.key)
      rows.value.push(r)
      added = true
    }
    if (!added) return
    if (sort) rows.value.sort((a, b) => a.at.getTime() - b.at.getTime())
    if (rows.value.length > MAX_ROWS) {
      for (const r of rows.value.splice(0, rows.value.length - MAX_ROWS)) keys.delete(r.key)
    }
    triggerRef(rows)
  }

  async function fetchNode(nodeId: string, before?: number) {
    try {
      const page = await nodeApi(nodeId)<LogPage>('/logs', {
        query: { limit: PAGE, level: level.value, before },
      })
      logFiles.set(nodeId, page.log_file)
      if (page.lines.length < PAGE) exhausted.add(nodeId)
      add(page.lines.map((l) => fromLine(l, nodeId)), true)
    } catch {
      // An older node without /logs, or one that just went away: it simply has no lines.
      exhausted.add(nodeId)
    }
  }

  function takeEvents() {
    const fresh = events.entries.filter((e) => e.id > lastEventId)
    if (!fresh.length) return
    lastEventId = fresh[fresh.length - 1]!.id
    add(fresh.map(fromEntry), false)
  }

  function send() {
    if (!socket || socket.readyState !== WebSocket.OPEN) return
    const sub: LogSubscription = { nodes: nodeIds.value, level: level.value }
    socket.send(JSON.stringify(sub))
  }

  function connect() {
    if (socket || reconnectTimer || needsSignIn.value || users === 0) return
    const proto = location.protocol === 'https:' ? 'wss' : 'ws'
    const ws = new WebSocket(`${proto}://${location.host}/ws/logs`)
    socket = ws
    ws.addEventListener('open', () => {
      attempt = 0
      send()
    })
    ws.addEventListener('message', (ev) => {
      if (typeof ev.data !== 'string') return
      const line = JSON.parse(ev.data) as LogLine
      if (line.node_id) add([fromLine(line, line.node_id)], false)
    })
    ws.addEventListener('close', () => {
      socket = null
      if (users === 0) return
      const delay = Math.min(BASE_DELAY * 2 ** attempt, MAX_DELAY)
      attempt++
      reconnectTimer = setTimeout(() => {
        reconnectTimer = null
        connect()
        // Lines logged while disconnected.
        for (const id of nodeIds.value) void fetchNode(id)
      }, delay)
    })
    ws.addEventListener('error', () => ws.close())
  }

  function reset() {
    rows.value = []
    keys.clear()
    exhausted.clear()
    lastEventId = 0
    takeEvents()
  }

  async function reload() {
    reset()
    send()
    await Promise.all(nodeIds.value.map((id) => fetchNode(id)))
  }

  /** Start streaming; call `close` when the view goes. */
  function open() {
    if (users++ > 0) return
    connect()
    void reload()
    stopWatches = [
      // The ref itself: a full log stays at the same length as it moves on.
      watch(storeToRefs(events).entries, takeEvents),
      watch(nodeIds, (ids, old) => {
        send()
        for (const id of ids) if (!old.includes(id)) void fetchNode(id)
      }),
      watch(level, reload),
    ]
  }

  function close() {
    if (--users > 0) return
    for (const stop of stopWatches) stop()
    stopWatches = []
    if (reconnectTimer) clearTimeout(reconnectTimer)
    reconnectTimer = null
    socket?.close()
    socket = null
  }

  /** The next page back for every node (or one) that has more. */
  async function loadOlder(nodeId: string | null) {
    loadingOlder.value = true
    try {
      const ids = (nodeId ? [nodeId] : nodeIds.value).filter((id) => !exhausted.has(id))
      await Promise.all(
        ids.map((id) => {
          const oldest = rows.value.find((r) => r.node_id === id && r.source === 'node' && r.seq)
          return fetchNode(id, oldest?.seq)
        }),
      )
    } finally {
      loadingOlder.value = false
    }
  }

  /** Clears the view, and the activity events with it. */
  function clear() {
    events.clear()
    reset()
  }

  return { rows, level, logFiles, exhausted, loadingOlder, open, close, loadOlder, clear }
})
