<script setup lang="ts">
import { X } from '@lucide/vue'
import { useViewKeys } from '@/composables/useViewKeys'
import { Button } from '@/components/ui/button'

/**
 * "N selected · actions · Clear", pinned to the bottom of the scrolling view while
 * anything is selected. Esc clears, unless the page's own Esc handler (registered
 * first) already used it and called `preventDefault()`.
 */
const props = defineProps<{ count: number }>()
const emit = defineEmits<{ clear: [] }>()

useViewKeys((e) => {
  if (e.key === 'Escape' && props.count && !e.defaultPrevented) emit('clear')
})
</script>

<template>
  <Transition
    enter-active-class="animate-in fade-in slide-in-from-bottom-2 duration-150"
    leave-active-class="animate-out fade-out slide-out-to-bottom-2 duration-100"
  >
    <div
      v-if="count"
      role="toolbar"
      aria-label="Selected items"
      class="sticky bottom-3 z-20 mx-auto mt-3 w-fit max-w-full flex flex-wrap items-center gap-2 rounded-lg border border-border bg-popover px-3 py-1.5 text-xs shadow-lg"
    >
      <span class="num font-medium text-primary whitespace-nowrap">{{ count }} selected</span>
      <div class="w-px h-4 bg-border" />
      <slot />
      <div class="w-px h-4 bg-border" />
      <Button variant="ghost" size="sm" class="h-7 gap-1 text-xs" title="Clear selection (Esc)" @click="emit('clear')">
        <X class="size-3.5" /> Clear
      </Button>
    </div>
  </Transition>
</template>
