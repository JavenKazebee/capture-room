<script setup lang="ts">
import type { FieldHelp } from '@/lib/fieldHelp'
import HelpTip from '@/components/common/HelpTip.vue'

defineProps<{
  label?: string
  /** Shown behind a `?` beside the label. */
  help?: FieldHelp
  /** Passed to the help popover: what the field resolves to right now. */
  helpNote?: string
}>()
</script>

<template>
  <label class="flex flex-col gap-1">
    <span class="flex items-center gap-1 text-xs text-muted-foreground">
      <slot name="label">{{ label }}</slot>
      <HelpTip v-if="help" :help="help" :note="helpNote" />
    </span>
    <slot />
  </label>
</template>
