<script setup lang="ts" generic="T extends string | number | null">
import type { HTMLAttributes } from 'vue'
import { cn } from '@/lib/utils'

const props = defineProps<{
  class?: HTMLAttributes['class']
}>()

// With `v-model.number`, a blank field becomes null and anything else a number.
const [model, modifiers] = defineModel<T, 'number'>({
  set: (value) => {
    if (!modifiers.number || typeof value !== 'string') return value
    return (value.trim() === '' ? null : Number(value)) as T
  },
})
</script>

<template>
  <input
    v-bind="$attrs"
    :value="model ?? ''"
    :class="cn(
      'flex h-7 w-full rounded-md border border-input bg-transparent px-2.5 py-1 text-xs shadow-sm transition-colors',
      'placeholder:text-muted-foreground',
      'focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
      'disabled:cursor-not-allowed disabled:opacity-50',
      props.class,
    )"
    @input="model = ($event.target as HTMLInputElement).value as T"
  />
</template>
