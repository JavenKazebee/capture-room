import { computed, onScopeDispose, ref, watch, type Ref } from 'vue'
import { thumbnailFrames, watchThumbnail } from '@/composables/useThumbnails'
import type { Source } from '@/stores/sources'

/**
 * `source`'s live thumbnail (monitored sources only), while this is mounted
 * and, given `el`, while that element is on screen.
 */
export function useThumbnail(source: Readonly<Ref<Source | null>>, el?: Readonly<Ref<HTMLElement | null>>) {
  const onScreen = ref(!el)
  if (el) {
    const observer = new IntersectionObserver((entries) => {
      onScreen.value = entries.some((e) => e.isIntersecting)
    })
    watch(
      el,
      (node, old) => {
        if (old) observer.unobserve(old)
        if (node) observer.observe(node)
      },
      { immediate: true },
    )
    onScopeDispose(() => observer.disconnect())
  }

  const key = computed(() => (source.value && onScreen.value ? source.value.key : null))
  let unwatch: (() => void) | null = null
  watch(
    key,
    () => {
      unwatch?.()
      unwatch = source.value && key.value ? watchThumbnail(source.value.node_id, source.value.id) : null
    },
    { immediate: true },
  )
  onScopeDispose(() => unwatch?.())

  const failed = ref(false)
  const src = computed(() => (key.value && !failed.value ? (thumbnailFrames.get(key.value) ?? null) : null))
  // A broken frame hides until the next one.
  watch(
    () => key.value && thumbnailFrames.get(key.value),
    () => (failed.value = false),
  )

  return { src, onError: () => (failed.value = true) }
}
