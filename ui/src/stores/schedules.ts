import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api } from '@/composables/useApi'
import { onReconnect } from '@/lib/reconnect'
import { useNodesStore } from '@/stores/nodes'
import type { ScheduleDto } from '@/types/generated/ScheduleDto'
import type { ScheduleInput } from '@/types/generated/ScheduleInput'
import type { ScheduleRunDto } from '@/types/generated/ScheduleRunDto'

export const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'] as const

/** Schedules, kept on (and run by) the controller. Empty on a plain node. */
export const useSchedulesStore = defineStore('schedules', () => {
  const nodes = useNodesStore()
  const schedules = ref<ScheduleDto[]>([])
  const loaded = ref(false)

  function upsert(s: ScheduleDto) {
    const idx = schedules.value.findIndex((x) => x.id === s.id)
    if (idx === -1) schedules.value.push(s)
    else schedules.value[idx] = s
    schedules.value.sort((a, b) => a.name.localeCompare(b.name))
  }

  async function load() {
    if (!nodes.isController) {
      schedules.value = []
      loaded.value = true
      return
    }
    schedules.value = await api<ScheduleDto[]>('/schedules').catch(() => [])
    loaded.value = true
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
    } catch {
      schedules.value = schedules.value.filter((s) => s.id !== id)
    }
  }

  return { schedules, loaded, load, create, update, remove, stop, runs, refresh }
})
