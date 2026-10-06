<script setup lang="ts">
import { Keyboard } from '@lucide/vue'
import { Kbd } from '@/components/ui/kbd'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'

/**
 * A view's toolbar row. Every page lays it out the same way:
 * title and count, then `#actions` (what you do here, primary first), then,
 * pushed right, the default slot's filters, a divider, and `#view` options
 * (columns, overlays, sizing). Keyboard shortcuts sit in a tooltip by the title.
 */
defineProps<{
  title: string
  count?: number | string
  /** Shortcut rows for the keyboard tooltip: keys, then what they do. */
  shortcuts?: [keys: string[], action: string][]
}>()
</script>

<template>
  <div class="flex flex-wrap items-center gap-x-4 gap-y-2 min-h-10 px-4 py-1.5 border-b border-border shrink-0">
    <div class="flex items-center gap-2 min-w-0">
      <h1 class="text-sm font-semibold tracking-tight">{{ title }}</h1>
      <span v-if="count !== undefined" class="num text-xs text-muted-foreground">{{ count }}</span>
      <Tooltip v-if="shortcuts?.length">
        <TooltipTrigger as-child>
          <button class="size-5 grid place-items-center rounded text-muted-foreground hover:text-foreground" aria-label="Keyboard shortcuts">
            <Keyboard class="size-3.5" />
          </button>
        </TooltipTrigger>
        <!-- A column of rows, not the tooltip's default single line. -->
        <TooltipContent side="bottom" align="start" class="grid grid-cols-[auto_1fr] items-center gap-x-3 gap-y-1.5 max-w-sm py-2 has-data-[slot=kbd]:pr-3">
          <template v-for="[keys, action] in shortcuts" :key="action">
            <span class="flex gap-0.5"><Kbd v-for="k in keys" :key="k">{{ k }}</Kbd></span>
            <span>{{ action }}</span>
          </template>
        </TooltipContent>
      </Tooltip>
    </div>

    <div v-if="$slots.actions" class="flex flex-wrap items-center gap-2">
      <slot name="actions" />
    </div>

    <div class="flex-1" />

    <div v-if="$slots.default" class="flex flex-wrap items-center gap-2">
      <slot />
    </div>
    <div v-if="$slots.default && $slots.view" class="w-px h-5 bg-border" />
    <div v-if="$slots.view" class="flex flex-wrap items-center gap-3">
      <slot name="view" />
    </div>
  </div>
</template>
