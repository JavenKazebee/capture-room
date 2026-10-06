<script setup lang="ts">
/** A toolbar control with its label in front, in the one label style toolbars use. */
defineProps<{ label: string }>()
</script>

<template>
  <label class="flex items-center gap-1.5 text-xs text-muted-foreground whitespace-nowrap">
    {{ label }}
    <slot />
  </label>
</template>
