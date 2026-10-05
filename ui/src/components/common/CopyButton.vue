<script setup lang="ts">
import { useClipboard } from '@vueuse/core'
import { Check, Copy } from '@lucide/vue'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'

/** Copies `value` — for IDs and paths shown in full elsewhere. `md` matches other row icon buttons. */
const props = withDefaults(defineProps<{ value: string; label?: string; size?: 'sm' | 'md' }>(), { label: 'Copy', size: 'sm' })
defineOptions({ inheritAttrs: false })
const { copy, copied } = useClipboard({ legacy: true })
</script>

<template>
  <Tooltip>
    <TooltipTrigger as-child>
      <button
        v-bind="$attrs"
        type="button"
        :class="props.size === 'md' ? 'size-6' : 'size-5'"
        class="inline-flex shrink-0 items-center justify-center rounded text-muted-foreground hover:text-foreground hover:bg-accent"
        :aria-label="props.label"
        @click.stop="copy(props.value)"
      >
        <Check v-if="copied" :class="props.size === 'md' ? 'size-3.5' : 'size-3'" class="text-success" />
        <Copy v-else :class="props.size === 'md' ? 'size-3.5' : 'size-3'" />
      </button>
    </TooltipTrigger>
    <TooltipContent>{{ copied ? 'Copied' : props.label }}</TooltipContent>
  </Tooltip>
</template>
