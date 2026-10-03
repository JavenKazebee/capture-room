<script setup lang="ts">
import { cn } from '@/lib/utils'

/** A small status light. `tally` pulses — reserved for live recording. */
const props = defineProps<{
  status: 'ok' | 'warn' | 'error' | 'off' | 'info' | 'tally'
  class?: string
}>()

const colors = {
  ok: 'bg-success',
  warn: 'bg-warning',
  error: 'bg-destructive',
  off: 'bg-muted-foreground/50',
  info: 'bg-info',
  tally: 'bg-tally animate-tally',
}
</script>

<template>
  <span :class="cn('inline-block size-2 shrink-0 rounded-full', colors[props.status], props.class)" />
</template>
