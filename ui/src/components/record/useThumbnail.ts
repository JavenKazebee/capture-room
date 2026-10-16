import { computed, ref, watch, type Ref } from 'vue'
import { thumbnailUrl } from '@/composables/useApi'
import { thumbnailSeqs, type Source } from '@/stores/sources'

/** Bumped by each `thumbnail.updated` (monitored sources only); a failed load retries on the next bump. */
export function useThumbnail(source: Readonly<Ref<Source | null>>) {
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
