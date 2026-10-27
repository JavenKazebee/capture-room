import { computed, ref, type MaybeRefOrGetter, toValue } from 'vue'
import { watchDebounced } from '@vueuse/core'
import { useNodesStore } from '@/stores/nodes'
import { useStorageStore } from '@/stores/storage'

/**
 * The reachable nodes among `nodeIds` that don't have the folder `path` (a plain path, `~` for
 * each node's home). Checked a moment after either changes; nodes that can't answer aren't listed.
 */
export function useFolderCheck(path: MaybeRefOrGetter<string>, nodeIds: MaybeRefOrGetter<string[]>) {
  const nodes = useNodesStore()
  const storage = useStorageStore()
  const missing = ref<string[]>([])
  /** Only the latest check's answer is kept. */
  let checks = 0

  const ids = computed(() => toValue(nodeIds).filter((id) => nodes.reachable.some((n) => n.id === id)))

  // Keyed as a string: the node list refreshes often without changing which nodes these are.
  watchDebounced(
    () => [toValue(path), ...ids.value].join('\n'),
    async () => {
      const mine = ++checks
      const dir = toValue(path)
      const targets = ids.value
      const found = await Promise.all(targets.map((id) => storage.folderExists(id, dir)))
      if (mine === checks) missing.value = targets.filter((_, i) => found[i] === false)
    },
    { debounce: 500, immediate: true },
  )

  return { missing }
}
