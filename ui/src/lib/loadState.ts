import { reactive, ref } from 'vue'
import { errorMessage } from '@/composables/useApi'

/**
 * A load's outcome, kept apart from its data so a failed load never reads as "nothing
 * there": the data keeps its last good value and `error` says why it may be stale.
 * `what` names the data in the error: `Couldn't load presets: …`.
 */
export function loadState(what: string) {
  /** Whether a load has succeeded at least once. */
  const loaded = ref(false)
  const error = ref<string | null>(null)

  /** Run `load`, which assigns the data only once it has it. Never throws: `undefined` if it failed. */
  async function track<T>(load: () => Promise<T>): Promise<T | undefined> {
    try {
      const value = await load()
      loaded.value = true
      error.value = null
      return value
    } catch (e) {
      error.value = `Couldn’t load ${what}: ${errorMessage(e, 'request failed')}`
      return undefined
    }
  }

  return { loaded, error, track }
}

/** The same per node: which nodes' last load failed, and why. */
export function nodeLoadErrors() {
  const errors = reactive(new Map<string, string>())

  async function track<T>(nodeId: string, load: () => Promise<T>): Promise<T | undefined> {
    try {
      const value = await load()
      errors.delete(nodeId)
      return value
    } catch (e) {
      errors.set(nodeId, errorMessage(e, 'request failed'))
      return undefined
    }
  }

  /** Forget nodes no longer loaded, so a removed node's failure doesn't linger. */
  function prune(ids: string[]) {
    for (const id of errors.keys()) if (!ids.includes(id)) errors.delete(id)
  }

  /** One line for every failed node: `Couldn't load sources from A, B: …`. */
  function message(what: string, nameOf: (id: string) => string): string | null {
    if (!errors.size) return null
    const names = [...errors.keys()].map(nameOf).join(', ')
    const details = [...new Set(errors.values())]
    return `Couldn’t load ${what} from ${names}${details.length === 1 ? `: ${details[0]}` : ''}`
  }

  return { errors, track, prune, message }
}
