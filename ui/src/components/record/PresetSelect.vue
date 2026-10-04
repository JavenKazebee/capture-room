<script setup lang="ts">
import { computed, type HTMLAttributes } from 'vue'
import { useRouter } from 'vue-router'
import { Plus } from '@lucide/vue'
import { cn } from '@/lib/utils'
import { Select, SelectContent, SelectItem, SelectSeparator, SelectTrigger, SelectValue } from '@/components/ui/select'

/**
 * A preset picker: the given presets, then a "New preset…" item that opens the
 * presets page with a fresh draft instead of selecting anything. Once saved,
 * that preset is applied to the `feeds` (source keys) this picker is for.
 */
const props = defineProps<{
  options: readonly { value: string; label: string }[]
  feeds: string[]
  disabled?: boolean
  class?: HTMLAttributes['class']
}>()

const model = defineModel<string>({ required: true })
const router = useRouter()

const NEW = '__new_preset__'

const value = computed({
  get: () => model.value,
  set: (v: string) => {
    if (v === NEW) router.push({ name: 'presets', query: { new: '1', for: props.feeds } })
    else model.value = v
  },
})
const selectedLabel = computed(() => props.options.find((o) => o.value === model.value)?.label)
</script>

<template>
  <Select v-model="value" :disabled="disabled">
    <SelectTrigger :class="cn('w-full min-w-0 overflow-hidden *:data-[slot=select-value]:truncate', $props.class)">
      <SelectValue>{{ selectedLabel }}</SelectValue>
    </SelectTrigger>
    <SelectContent>
      <SelectItem v-for="opt in options" :key="opt.value" :value="opt.value">
        {{ opt.label }}
      </SelectItem>
      <SelectSeparator />
      <SelectItem :value="NEW" class="text-muted-foreground">
        <Plus class="size-3.5" /> New preset…
      </SelectItem>
    </SelectContent>
  </Select>
</template>
