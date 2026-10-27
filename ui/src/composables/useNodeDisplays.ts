import type { Ref } from 'vue'
import { useNodeResource } from '@/composables/useNodeResource'
import { useSourcesStore } from '@/stores/sources'

/** A node's screens, for display outputs, loaded when `nodeId` is set or changes. */
export function useNodeDisplays(nodeId: Ref<string>) {
  const store = useSourcesStore()
  const { data, ...rest } = useNodeResource(nodeId, store.listDisplays, () => [], 'Could not list the screens.')
  return { displays: data, ...rest }
}
