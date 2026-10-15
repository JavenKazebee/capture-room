import { ref, watch, type Ref } from 'vue'
import { useSourcesStore } from '@/stores/sources'
import { errorMessage } from '@/composables/useApi'
import type { DeviceDto } from '@/types/generated/DeviceDto'

/** A node's capture devices, loaded when `nodeId` is set or changes. */
export function useNodeDevices(nodeId: Ref<string>) {
  const store = useSourcesStore()
  const devices = ref<DeviceDto[]>([])
  const loading = ref(false)
  const error = ref<string | null>(null)
  /** The latest request: a refresh for the same node shares it. */
  let pending: { node: string; done: Promise<void> } | null = null

  /** Resolves once the node's devices are in (or another node was picked meanwhile). */
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
      const list = await store.listDevices(request.node)
      // A later request answers instead: its node is the one shown.
      if (pending === request) devices.value = list
    } catch (e) {
      if (pending === request) {
        error.value = errorMessage(e, 'Could not list the devices.')
        devices.value = []
      }
    } finally {
      if (pending === request) {
        pending = null
        loading.value = false
      }
    }
  }

  watch(nodeId, refresh, { immediate: true })
  return { devices, loading, error, refresh }
}
