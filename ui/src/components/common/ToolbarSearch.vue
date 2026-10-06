<script setup lang="ts">
import { Search } from '@lucide/vue'
import { Input } from '@/components/ui/input'

/** The text filter every toolbar uses. */
withDefaults(defineProps<{ placeholder?: string }>(), { placeholder: 'Filter…' })
const model = defineModel<string>({ required: true })
</script>

<template>
  <div class="relative">
    <Search class="absolute left-2 top-1/2 -translate-y-1/2 size-3.5 text-muted-foreground pointer-events-none" />
    <Input v-model="model" :placeholder="placeholder" class="h-7 w-44 pl-7 text-xs" />
  </div>
</template>
