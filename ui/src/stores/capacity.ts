import { defineStore } from 'pinia'
import { reactive } from 'vue'
import { nodeApi } from '@/composables/useApi'
import type { BenchmarkRequest } from '@/types/generated/BenchmarkRequest'
import type { BenchmarkRunDto } from '@/types/generated/BenchmarkRunDto'
import type { CapacityCheckDto } from '@/types/generated/CapacityCheckDto'
import type { MediaInfo } from '@/types/generated/MediaInfo'
import type { NodeCapacityDto } from '@/types/generated/NodeCapacityDto'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'
import { fpsLabel } from '@/lib/sourceFormat'

/** Recording time left on a volume below which the UI warns. */
export const LOW_TIME_SECS = 3600

/** `1080p30`, `2160p59.94`; other sizes as `1440×1080 30`. */
export function formatLabel(m: Pick<MediaInfo, 'width' | 'height' | 'fps_num' | 'fps_den'>) {
  const rate = fpsLabel([m.fps_num, m.fps_den], true)
  const standard = [480, 576, 720, 1080, 1440, 2160, 4320].includes(m.height) && m.width >= m.height
  return standard ? `${m.height}p${rate}` : `${m.width}×${m.height} ${rate}`
}

/**
 * Benchmarks and capacity per node: what each benchmarked setup sustains, how
 * loaded the node is, the benchmark running now, and past runs.
 */
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
      body: { outputs, source_ids: sourceIds },
    })
  }

  return { capacity, runs, load, loadRuns, applyRun, start, cancel, remove, check }
})
