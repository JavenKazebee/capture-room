import type { Ref } from 'vue'
import { useNodeResource } from '@/composables/useNodeResource'
import { useSourcesStore } from '@/stores/sources'

/** A node's capture devices, loaded when `nodeId` is set or changes. */
export function useNodeDevices(nodeId: Ref<string>) {
  const store = useSourcesStore()
  const { data, ...rest } = useNodeResource(nodeId, store.listDevices, () => [], 'Could not list the devices.')
  return { devices: data, ...rest }
}
