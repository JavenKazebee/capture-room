import { ref, watch, type Ref } from 'vue'
import { useSourcesStore } from '@/stores/sources'
import { errorMessage } from '@/composables/useApi'
import type { DecklinkStatusDto } from '@/types/generated/DecklinkStatusDto'

/** A node's Decklink cards, loaded when `nodeId` is set or changes. */
export function useNodeDecklink(nodeId: Ref<string>) {
  const store = useSourcesStore()
  const status = ref<DecklinkStatusDto>({ unavailable: null, cards: [] })
  const loading = ref(false)
  const error = ref<string | null>(null)

  async function refresh() {
    const node = nodeId.value
    if (!node) return
    loading.value = true
    error.value = null
    try {
      const s = await store.listDecklink(node)
      if (nodeId.value === node) status.value = s
    } catch (e) {
      if (nodeId.value === node) {
        error.value = errorMessage(e, 'Could not list the Decklink cards.')
        status.value = { unavailable: null, cards: [] }
      }
    } finally {
      if (nodeId.value === node) loading.value = false
    }
  }

  watch(nodeId, refresh, { immediate: true })
  return { status, loading, error, refresh }
}
