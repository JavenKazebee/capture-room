<script setup lang="ts">
/** The live-recording marker: pulsing tally dot, REC, and optional elapsed time. */
defineProps<{ duration?: string }>()
</script>

<template>
  <span
    class="inline-flex items-center gap-1.5 rounded-sm bg-tally px-1.5 py-0.5 text-[10px] font-bold uppercase tracking-wider text-tally-foreground"
  >
    <span class="size-1.5 rounded-full bg-current animate-tally" />
    Rec
    <span v-if="duration" class="num font-medium normal-case tracking-normal">{{ duration }}</span>
  </span>
</template>
