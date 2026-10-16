<script setup lang="ts" generic="T extends string | number">
import { computed, type HTMLAttributes } from 'vue'
import { cn } from '@/lib/utils'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'

/** A Select over fixed options; a `disabled` option shows its `reason`. */
const props = defineProps<{
  options: readonly { value: T; label: string; disabled?: boolean; reason?: string }[]
  disabled?: boolean
  class?: HTMLAttributes['class']
}>()

const model = defineModel<T>({ required: true })
// SelectValue alone keeps the label from when the menu was last open, so a label that
// depends on other fields ("Auto (AAC)") would go stale.
const selectedLabel = computed(() => props.options.find((o) => o.value === model.value)?.label)
</script>

<template>
  <Select v-model="model" :disabled="disabled">
    <SelectTrigger :class="cn('w-full min-w-0 overflow-hidden *:data-[slot=select-value]:truncate', $props.class)">
      <SelectValue>{{ selectedLabel }}</SelectValue>
    </SelectTrigger>
    <SelectContent>
      <SelectItem v-for="opt in options" :key="opt.value" :value="opt.value" :disabled="opt.disabled">
        {{ opt.label }}
        <span v-if="opt.disabled && opt.reason" class="ml-2 text-muted-foreground">— {{ opt.reason }}</span>
      </SelectItem>
    </SelectContent>
  </Select>
</template>
