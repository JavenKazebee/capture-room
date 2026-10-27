import { toast } from 'vue-sonner'
import { sourceKey } from '@/composables/useApi'
import { plural } from '@/lib/format'
import { useEventsStore } from '@/stores/events'
import { useNodesStore } from '@/stores/nodes'
import { usePlayoutStore } from '@/stores/playout'
import { DEFAULT_PRESET } from '@/stores/presets'
import { totalDropped, useRecordingsStore } from '@/stores/recordings'
import { useSourcesStore } from '@/stores/sources'
import type { NodeEvent } from '@/types/generated/NodeEvent'

/** Record the notable events in the event log, before the stores apply them. */
export function logEvent(event: NodeEvent) {
  const log = useEventsStore().log
  const node_id = event.node_id
  const nodes = useNodesStore()
  const source = (id: string) => useSourcesStore().nameOf(node_id, id)

  switch (event.type) {
    case 'schedule.updated': {
      const run = event.run
      if (!run) break
      const name = `Schedule “${event.name}”`
      const failures = run.targets.filter((t) => t.error)
      const targetName = (t: (typeof failures)[number]) => useSourcesStore().nameOf(t.node_id, t.id)
      const detail = failures.map((t) => `${targetName(t)}: ${t.error}`).join('; ') || undefined
      if (run.status === 'skipped') {
        log('warn', `${name} skipped: its time passed while the controller was off`, { node_id })
        toast.warning(`${name} skipped`)
      } else if (run.status === 'failed') {
        log('error', `${name} couldn't start anything`, { node_id, detail })
        toast.error(`${name} couldn't start`, { description: detail })
      } else if (run.stopped_at) {
        log('info', `${name} ended`, { node_id })
      } else {
        const partial = failures.length > 0
        log(partial ? 'warn' : 'rec', `${name} started${partial ? `, ${plural(failures.length, 'target')} failed` : ''}`, { node_id, detail })
        if (partial) toast.warning(`${name} started, but not everything`, { description: detail })
        else toast.success(`${name} started`)
      }
      break
    }
    case 'node.online':
      log('info', `Node online: ${nodes.nameOf(event.peer_id)}`, { node_id: event.peer_id })
      break
    case 'node.offline':
      log('warn', `Node offline: ${nodes.nameOf(event.peer_id)}`, { node_id: event.peer_id })
      break
    case 'node.removed':
      log('info', `Node removed: ${nodes.nameOf(event.peer_id)}`, { node_id: event.peer_id })
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
      const name = run.preset_name ?? DEFAULT_PRESET.name
      if (run.status === 'running') {
        if (run.feeds_running === 0) log('info', `Benchmark started: ${name}`, { node_id })
      } else if (run.status === 'completed') {
        const n = run.sustainable_feeds
        const atLimit = n >= run.max_feeds
        log('info', `Benchmark finished: ${name} sustains ${atLimit ? 'at least ' : ''}${plural(n, 'feed')}`, {
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
