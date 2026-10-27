import { onBeforeUnmount, onMounted, ref, type Ref } from 'vue'
import { onBeforeRouteLeave, useRouter } from 'vue-router'

/**
 * Ask before throwing away unsaved edits: on leaving the page, and on whatever else the
 * caller passes to `guarded`. Bind `confirmOpen` and `discard` to a `ConfirmDialog`.
 * Closing the tab gets the browser's own prompt, the only one it allows there.
 */
export function useUnsavedGuard(dirty: Ref<boolean>) {
  const router = useRouter()
  const confirmOpen = ref(false)
  // The dialog closes itself before its confirm fires, so closing must not drop this.
  let pending: (() => void) | null = null
  let leaving = false

  function guarded(fn: () => void) {
    if (!dirty.value) return fn()
    pending = fn
    confirmOpen.value = true
  }

  function discard() {
    const fn = pending
    pending = null
    fn?.()
  }

  onBeforeRouteLeave((to) => {
    if (leaving || !dirty.value) return true
    guarded(() => {
      leaving = true
      router.push(to.fullPath).finally(() => (leaving = false))
    })
    return false
  })

  function onBeforeUnload(e: BeforeUnloadEvent) {
    if (dirty.value) e.preventDefault()
  }
  onMounted(() => window.addEventListener('beforeunload', onBeforeUnload))
  onBeforeUnmount(() => window.removeEventListener('beforeunload', onBeforeUnload))

  return { confirmOpen, guarded, discard }
}
