<script setup lang="ts">
import { useClipboard } from '@vueuse/core'
import { Check, Copy } from '@lucide/vue'

/** Copies `value` — for IDs and paths shown in full elsewhere. */
const props = defineProps<{ value: string }>()
const { copy, copied } = useClipboard({ legacy: true })
</script>

<template>
  <button
    type="button"
    class="inline-flex size-5 shrink-0 items-center justify-center rounded text-muted-foreground hover:text-foreground hover:bg-accent"
    :title="copied ? 'Copied' : 'Copy'"
    @click.stop="copy(props.value)"
  >
    <Check v-if="copied" class="size-3 text-success" />
    <Copy v-else class="size-3" />
  </button>
</template>
