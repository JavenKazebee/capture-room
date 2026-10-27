<script setup lang="ts">
import { X } from '@lucide/vue'

/** One of a channel's outputs: its type, the main control beside it, and its fields below. */
defineProps<{ label: string; index: number }>()
const emit = defineEmits<{ remove: [] }>()
</script>

<template>
  <div class="flex flex-col gap-2 rounded-md border border-border px-2.5 py-1.5">
    <div class="flex items-center gap-2">
      <span class="text-[10px] font-semibold uppercase tracking-wide text-muted-foreground w-12 shrink-0">{{ label }}</span>
      <slot name="control" />
      <button
        type="button"
        class="size-7 grid place-items-center rounded text-muted-foreground hover:text-foreground hover:bg-accent"
        :aria-label="`Remove output ${index + 1}`"
        @click="emit('remove')"
      >
        <X class="size-3.5" />
      </button>
    </div>
    <slot />
  </div>
</template>
