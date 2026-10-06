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

  async function refresh() {
    if (!nodeId.value) return
    loading.value = true
    error.value = null
    try {
      devices.value = await store.listDevices(nodeId.value)
    } catch (e) {
      error.value = errorMessage(e, 'Could not list the devices.')
      devices.value = []
    } finally {
      loading.value = false
    }
  }

  watch(nodeId, refresh, { immediate: true })
  return { devices, loading, error, refresh }
}
