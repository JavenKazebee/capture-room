<script setup lang="ts">
defineProps<{ label?: string }>()
</script>

<template>
  <label class="flex flex-col gap-1">
    <span class="text-xs text-muted-foreground">
      <slot name="label">{{ label }}</slot>
    </span>
    <slot />
  </label>
</template>
