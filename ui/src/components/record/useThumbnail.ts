import { computed, ref, watch, type Ref } from 'vue'
import { thumbnailUrl } from '@/composables/useApi'
import { thumbnailSeqs, type Source } from '@/stores/sources'

/**
 * A source's live thumbnail URL. Bumped by every `thumbnail.updated` event,
 * which only monitored sources get; a failed load is retried by the next bump.
 */
export function useThumbnail(source: Ref<Source | null>) {
  const seq = computed(() => (source.value ? (thumbnailSeqs.get(source.value.key) ?? 0) : 0))
  const failed = ref(false)
  watch(seq, () => (failed.value = false))

  const src = computed(() =>
    source.value && seq.value > 0 && !failed.value
      ? `${thumbnailUrl(source.value.node_id, source.value.id)}?t=${seq.value}`
      : null,
  )

  return { src, onError: () => (failed.value = true) }
}
