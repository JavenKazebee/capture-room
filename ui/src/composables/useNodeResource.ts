import { ref, watch, type Ref } from 'vue'
import { errorMessage } from '@/composables/useApi'

/**
 * Something listed from one node, loaded when `nodeId` is set or changes. Answers for a node
 * no longer picked are dropped, so a slow node never overwrites the one shown.
 */
export function useNodeResource<T>(
  nodeId: Ref<string>,
  fetch: (nodeId: string) => Promise<T>,
  empty: () => T,
  failure: string,
) {
  const data = ref(empty()) as Ref<T>
  const loading = ref(false)
  const error = ref<string | null>(null)
  /** The latest request: a refresh for the same node shares it. */
  let pending: { node: string; done: Promise<void> } | null = null

  /** Resolves once the node's answer is in (or another node was picked meanwhile). */
  function refresh(): Promise<void> {
    const node = nodeId.value
    if (!node) return Promise.resolve()
    if (pending?.node === node) return pending.done
    const request = { node, done: Promise.resolve() }
    pending = request
    request.done = load(request)
    return request.done
  }

  async function load(request: { node: string }) {
    loading.value = true
    error.value = null
    try {
      const value = await fetch(request.node)
      if (pending === request) data.value = value
    } catch (e) {
      if (pending === request) {
        error.value = errorMessage(e, failure)
        data.value = empty()
      }
    } finally {
      if (pending === request) {
        pending = null
        loading.value = false
      }
    }
  }

  watch(nodeId, refresh, { immediate: true })
  return { data, loading, error, refresh }
}
