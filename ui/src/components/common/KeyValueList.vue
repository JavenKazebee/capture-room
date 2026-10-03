<script setup lang="ts">
import CopyButton from './CopyButton.vue'

/** A dense label/value readout. `mono` values use the technical font; `copy` adds a copy button. */
defineProps<{
  items: { label: string; value: string | number | null | undefined; mono?: boolean; copy?: boolean }[]
}>()
</script>

<template>
  <dl class="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-xs">
    <template v-for="item in items" :key="item.label">
      <dt class="text-muted-foreground">{{ item.label }}</dt>
      <dd class="flex min-w-0 items-center gap-1">
        <span class="truncate" :class="item.mono && 'num'" :title="String(item.value ?? '')">
          {{ item.value ?? '—' }}
        </span>
        <CopyButton v-if="item.copy && item.value" :value="String(item.value)" />
      </dd>
    </template>
  </dl>
</template>
