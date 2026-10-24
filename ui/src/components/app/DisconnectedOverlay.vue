<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, useTemplateRef, watch } from 'vue'
import { FocusScope } from 'reka-ui'
import { Loader2, RefreshCw, Unplug } from '@lucide/vue'
import { needsSignIn } from '@/composables/useAuth'
import { reconnectAttempt, reconnectNow, wsStatus } from '@/composables/useWebSocket'
import { Button } from '@/components/ui/button'

/** How long the socket may be down before the whole app is covered; shorter blips just show the header chip. */
const GRACE_MS = 3_000

const down = computed(() => wsStatus.value !== 'connected' && !needsSignIn.value)
const shown = ref(false)
const retry = useTemplateRef<InstanceType<typeof Button>>('retry')

let timer: ReturnType<typeof setTimeout> | null = null

// Applies to the first connection too: a page load that connects within the
// grace period never sees the overlay.
watch(
  down,
  (isDown) => {
    if (timer) clearTimeout(timer)
    timer = null
    if (!isDown) shown.value = false
    else if (!shown.value) timer = setTimeout(() => (shown.value = true), GRACE_MS)
  },
  { immediate: true },
)
onBeforeUnmount(() => timer && clearTimeout(timer))

watch(shown, async (v) => {
  if (!v) return
  await nextTick()
  ;(retry.value?.$el as HTMLElement | undefined)?.focus()
})

const attempt = computed(() => Math.max(1, reconnectAttempt.value))
</script>

<template>
  <Transition
    enter-active-class="transition-opacity duration-200"
    leave-active-class="transition-opacity duration-300"
    enter-from-class="opacity-0"
    leave-to-class="opacity-0"
  >
    <!-- Keys stop here so page shortcuts and the dialogs underneath don't act while covered. -->
    <div
      v-if="shown"
      class="fixed inset-0 z-[100] grid place-items-center p-4 bg-background/60 backdrop-blur-sm"
      @keydown.stop
    >
      <FocusScope trapped loop as-child>
        <div
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="disconnected-title"
          aria-describedby="disconnected-desc"
          class="w-full max-w-sm rounded-xl bg-popover text-popover-foreground ring-1 ring-foreground/10 shadow-2xl p-5 text-center space-y-4"
        >
          <span class="mx-auto size-10 rounded-full bg-warning/15 text-warning grid place-items-center">
            <Unplug class="size-5" />
          </span>
          <div class="space-y-1.5">
            <h2 id="disconnected-title" class="text-sm font-semibold">Lost connection to the controller</h2>
            <p class="text-xs text-muted-foreground flex items-center justify-center gap-1.5" aria-live="polite">
              <Loader2 class="size-3 animate-spin" />
              Reconnecting… <span class="num">(attempt {{ attempt }})</span>
            </p>
          </div>
          <p id="disconnected-desc" class="text-xs/relaxed text-muted-foreground">
            Recordings keep running on the nodes. This page catches up as soon as it reconnects.
          </p>
          <Button ref="retry" class="w-full gap-1.5" @click="reconnectNow">
            <RefreshCw class="size-3.5" />
            Retry now
          </Button>
        </div>
      </FocusScope>
    </div>
  </Transition>
</template>
