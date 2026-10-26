import { FetchError } from 'ofetch'
import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api, errorMessage } from '@/composables/useApi'
import { loadState } from '@/lib/loadState'
import { onReconnect } from '@/lib/reconnect'
import { useEventsStore } from '@/stores/events'
import { useNodesStore } from '@/stores/nodes'
import type { ScheduleDto } from '@/types/generated/ScheduleDto'
import type { ScheduleInput } from '@/types/generated/ScheduleInput'
import type { ScheduleRunDto } from '@/types/generated/ScheduleRunDto'

export const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'] as const

/** Schedules, kept on (and run by) the controller. Empty on a plain node. */
export const useSchedulesStore = defineStore('schedules', () => {
  const nodes = useNodesStore()
  const schedules = ref<ScheduleDto[]>([])
  const { loaded, error, track } = loadState('schedules')

  function upsert(s: ScheduleDto) {
    const idx = schedules.value.findIndex((x) => x.id === s.id)
    if (idx === -1) schedules.value.push(s)
    else schedules.value[idx] = s
    schedules.value.sort((a, b) => a.name.localeCompare(b.name))
  }

  async function load() {
    await track(async () => {
      // Without its settings this instance may be a controller: don't report "no schedules".
      if (!nodes.self) await nodes.load()
      if (!nodes.self) throw new Error('this node’s settings are unknown')
      schedules.value = nodes.isController ? await api<ScheduleDto[]>('/schedules') : []
    })
  }

  onReconnect(load)

  async function create(input: ScheduleInput) {
    const s = await api<ScheduleDto>('/schedules', { method: 'POST', body: input })
    upsert(s)
    return s
  }

  async function update(id: string, input: ScheduleInput) {
    const s = await api<ScheduleDto>(`/schedules/${id}`, { method: 'PUT', body: input })
    upsert(s)
    return s
  }

  async function remove(id: string) {
    await api(`/schedules/${id}`, { method: 'DELETE' })
    schedules.value = schedules.value.filter((s) => s.id !== id)
  }

  async function stop(id: string) {
    upsert(await api<ScheduleDto>(`/schedules/${id}/stop`, { method: 'POST' }))
  }

  function runs(id: string, limit = 20) {
    return api<ScheduleRunDto[]>(`/schedules/${id}/runs`, { query: { limit } })
  }

  /** A schedule changed, or a run started or ended: reload it (or drop it). */
  async function refresh(id: string) {
    if (!loaded.value) return
    try {
      upsert(await api<ScheduleDto>(`/schedules/${id}`))
    } catch (e) {
      // A 404 means the schedule is gone: drop it. Anything else is transient:
      // keep the last known state until the next event or reload.
      if (e instanceof FetchError && e.statusCode === 404) {
        schedules.value = schedules.value.filter((s) => s.id !== id)
      } else {
        const detail = errorMessage(e, '')
        useEventsStore().log('warn', `Failed to refresh schedule ${id}`, { detail: detail || undefined })
      }
    }
  }

  return { schedules, loaded, error, load, create, update, remove, stop, runs, refresh }
})
