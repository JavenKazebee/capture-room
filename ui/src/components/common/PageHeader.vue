<script setup lang="ts">
/** A view's title row: title, optional count/description, toolbar on the right. */
defineProps<{ title: string; count?: number; description?: string }>()
</script>

<template>
  <div class="flex flex-wrap items-center gap-x-4 gap-y-2 min-h-12 px-4 py-2 border-b border-border">
    <div class="flex items-baseline gap-2 min-w-0">
      <h1 class="text-base font-semibold tracking-tight">{{ title }}</h1>
      <span v-if="count !== undefined" class="num text-xs text-muted-foreground">{{ count }}</span>
      <span v-if="description" class="text-xs text-muted-foreground truncate">{{ description }}</span>
    </div>
    <div class="flex-1" />
    <div class="flex flex-wrap items-center gap-2">
      <slot />
    </div>
  </div>
</template>
