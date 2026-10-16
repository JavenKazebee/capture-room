import { defineStore } from 'pinia'
import { reactive } from 'vue'
import { nodeApi } from '@/composables/useApi'
import type { BenchmarkRequest } from '@/types/generated/BenchmarkRequest'
import type { BenchmarkRunDto } from '@/types/generated/BenchmarkRunDto'
import type { CapacityCheckDto } from '@/types/generated/CapacityCheckDto'
import type { ChannelConfig } from '@/types/generated/ChannelConfig'
import type { NodeCapacityDto } from '@/types/generated/NodeCapacityDto'
import type { OutputConfig } from '@/types/generated/OutputConfig'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'
import type { ReplayCodec } from '@/types/generated/ReplayCodec'
import { DEFAULT_PRESET } from '@/stores/presets'
import { CODECS } from '@/stores/replay'
import { onReconnect } from '@/lib/reconnect'
import type { CapacityCheckRequest } from '@/types/generated/CapacityCheckRequest'

/** Recording time left on a volume below which the UI warns. */
export const LOW_TIME_SECS = 3600

/** The warning for a node at `load` of its benchmarked capacity (1 = all of it), if it's near or over. */
export function capacityWarningText(nodeName: string, load: number): string | null {
  const pct = Math.round(load * 100)
  if (pct > 100) return `${nodeName} is over its benchmarked capacity (${pct}%): expect dropped frames.`
  if (pct > 80) return `${nodeName} is near its benchmarked capacity (${pct}%).`
  return null
}

/** A channel benchmark's outputs: `NDI + SRT 8 Mb/s`, or `program only`. */
export function channelOutputsLabel(outputs: OutputConfig[]) {
  const parts = outputs.map((o) =>
    o.type === 'srt' ? `SRT ${+(o.bitrate_kbps / 1000).toFixed(1)} Mb/s` : o.type === 'ndi' ? 'NDI' : o.type,
  )
  return parts.length ? parts.join(' + ') : 'program only'
}

/** What a benchmark run or profile measured: a preset's outputs, replay buffers, or playout channels. */
export function setupLabel(s: { preset_name: string | null; replay: ReplayCodec | null; channel: OutputConfig[] | null }) {
  if (s.channel) return `Channel · ${channelOutputsLabel(s.channel)}`
  if (s.replay) return `Replay buffer · ${CODECS[s.replay].label}`
  return s.preset_name ?? DEFAULT_PRESET.name
}

/** Benchmarks and capacity per node: what each setup sustains, current load, the running benchmark and past runs. */
export const useCapacityStore = defineStore('capacity', () => {
  /** Per node id; `null` = failed to load. */
  const capacity = reactive(new Map<string, NodeCapacityDto | null>())
  /** Past runs per node, newest first; loaded on demand. */
  const runs = reactive(new Map<string, BenchmarkRunDto[]>())

  async function load(nodeId: string) {
    capacity.set(nodeId, await nodeApi(nodeId)<NodeCapacityDto>('/capacity').catch(() => null))
  }

  async function loadRuns(nodeId: string) {
    runs.set(nodeId, await nodeApi(nodeId)<BenchmarkRunDto[]>('/benchmarks'))
  }

  /** A run's state from `benchmark.updated`. */
  function applyRun(nodeId: string, run: BenchmarkRunDto) {
    const list = runs.get(nodeId)
    if (list) {
      const i = list.findIndex((r) => r.id === run.id)
      if (i === -1) list.unshift(run)
      else list[i] = run
    }
    const cap = capacity.get(nodeId)
    if (run.status === 'running') {
      if (cap) cap.running = run
    } else {
      // Finished: its result may change the node's profiles.
      load(nodeId)
    }
  }

  async function start(nodeId: string, req: BenchmarkRequest) {
    const run = await nodeApi(nodeId)<BenchmarkRunDto>('/benchmarks', { method: 'POST', body: req })
    applyRun(nodeId, run)
    return run
  }

  async function cancel(nodeId: string, id: string) {
    const run = await nodeApi(nodeId)<BenchmarkRunDto>(`/benchmarks/${id}/cancel`, { method: 'POST' })
    applyRun(nodeId, run)
  }

  async function remove(nodeId: string, id: string) {
    await nodeApi(nodeId)(`/benchmarks/${id}`, { method: 'DELETE' })
    const list = runs.get(nodeId)
    if (list) runs.set(nodeId, list.filter((r) => r.id !== id))
    await load(nodeId)
  }

  /** Would recording `sourceIds` with `outputs` fit on the node? */
  function check(nodeId: string, outputs: PresetOutputInput[], sourceIds: string[]) {
    return nodeApi(nodeId)<CapacityCheckDto>('/capacity/check', {
      method: 'POST',
      body: { outputs, source_ids: sourceIds, replay: null, channel: null } satisfies CapacityCheckRequest,
    })
  }

  /** Would giving `sourceIds` replay buffers in `codec` fit on the node? */
  function checkReplay(nodeId: string, codec: ReplayCodec, sourceIds: string[]) {
    return nodeApi(nodeId)<CapacityCheckDto>('/capacity/check', {
      method: 'POST',
      body: { outputs: [], source_ids: sourceIds, replay: codec, channel: null } satisfies CapacityCheckRequest,
    })
  }

  /** Would a playout channel like `channel` fit? `editing` is the channel it replaces, no longer counted. */
  function checkChannel(nodeId: string, channel: ChannelConfig, editing?: string) {
    return nodeApi(nodeId)<CapacityCheckDto>('/capacity/check', {
      method: 'POST',
      body: { outputs: [], source_ids: editing ? [editing] : [], replay: null, channel } satisfies CapacityCheckRequest,
    })
  }

  /** Fetch again whatever was loaded: after a reconnect, when events may have been missed. */
  async function reloadLoaded() {
    await Promise.all([
      ...[...capacity.keys()].map((id) => load(id)),
      ...[...runs.keys()].map((id) => loadRuns(id).catch(() => {})),
    ])
  }

  onReconnect(reloadLoaded)

  return { capacity, runs, load, loadRuns, applyRun, start, cancel, remove, check, checkReplay, checkChannel }
})
