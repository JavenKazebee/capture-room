import { defineStore } from 'pinia'
import { ref } from 'vue'

export interface Schedule {
  id: string
  node_id: string
  source_id: string
  preset_id: string
  start_at: string
  stop_at: string
  recurrence: string | null
  status: 'pending' | 'active' | 'completed' | 'error'
  created_at: string
}

export const useSchedulesStore = defineStore('schedules', () => {
  const schedules = ref<Schedule[]>([])

  function set(list: Schedule[]) {
    schedules.value = list
  }

  function upsert(schedule: Schedule) {
    const idx = schedules.value.findIndex((s) => s.id === schedule.id)
    if (idx === -1) schedules.value.push(schedule)
    else schedules.value[idx] = schedule
  }

  function remove(id: string) {
    schedules.value = schedules.value.filter((s) => s.id !== id)
  }

  return { schedules, set, upsert, remove }
})
