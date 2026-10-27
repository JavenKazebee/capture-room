import { defineStore } from 'pinia'
import { computed, shallowRef, triggerRef } from 'vue'

export type ActivityLevel = 'info' | 'warn' | 'error' | 'rec'

export interface LogEntry {
  id: number
  at: Date
  level: ActivityLevel
  /** The node the entry is about, when there is one. */
  node_id: string | null
  message: string
  detail?: string
}

const MAX_ENTRIES = 1000

/** Notable server events and failed UI actions, in memory for this page load. High-rate events are left out. */
export const useEventsStore = defineStore('events', () => {
  // A shallow ref over a mutated array — cheaper than deep reactivity for a log.
  const entries = shallowRef<LogEntry[]>([])
  let nextId = 1

  /** Errors logged since the log was last viewed. */
  const unseenErrors = shallowRef(0)

  function log(level: ActivityLevel, message: string, opts: { node_id?: string | null; detail?: string } = {}) {
    entries.value.push({ id: nextId++, at: new Date(), level, node_id: opts.node_id ?? null, message, detail: opts.detail })
    if (entries.value.length > MAX_ENTRIES) entries.value.splice(0, entries.value.length - MAX_ENTRIES)
    if (level === 'error') unseenErrors.value++
    triggerRef(entries)
  }

  function clear() {
    entries.value = []
    unseenErrors.value = 0
  }

  function markSeen() {
    unseenErrors.value = 0
  }

  const counts = computed(() => {
    const c = { info: 0, warn: 0, error: 0, rec: 0 }
    for (const e of entries.value) c[e.level]++
    return c
  })

  return { entries, unseenErrors, counts, log, clear, markSeen }
})
