import type { Ref } from 'vue'
import { useNodeResource } from '@/composables/useNodeResource'
import { useSourcesStore } from '@/stores/sources'

/** A node's Decklink cards, loaded when `nodeId` is set or changes. */
export function useNodeDecklink(nodeId: Ref<string>) {
  const store = useSourcesStore()
  const { data, ...rest } = useNodeResource(
    nodeId,
    store.listDecklink,
    () => ({ unavailable: null, cards: [] }),
    'Could not list the Decklink cards.',
  )
  return { status: data, ...rest }
}
