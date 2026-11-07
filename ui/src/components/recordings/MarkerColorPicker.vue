<script setup lang="ts">
import { MARKER_COLORS } from '@/lib/markers'
import type { MarkerColor } from '@/types/generated/MarkerColor'

const color = defineModel<MarkerColor>({ required: true })
</script>

<template>
  <div class="flex items-center gap-1.5" role="radiogroup" aria-label="Marker colour">
    <button
      v-for="c in MARKER_COLORS"
      :key="c.value"
      type="button"
      role="radio"
      :aria-checked="color === c.value"
      :aria-label="c.label"
      :title="c.label"
      class="size-5 rounded-full border-2 transition-transform focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
      :class="color === c.value ? 'border-foreground scale-110' : 'border-transparent hover:scale-110'"
      :style="{ backgroundColor: c.swatch }"
      @click="color = c.value"
    />
  </div>
</template>
