<script setup lang="ts">
import { ref } from 'vue'
import { CircleHelp } from '@lucide/vue'
import type { FieldHelp } from '@/lib/fieldHelp'
import { Popover, PopoverAnchor, PopoverContent } from '@/components/ui/popover'

/**
 * A `?` beside a field label that explains the field. Opens on hover, and on
 * click or tap; a click keeps it open until something else is clicked, so it
 * works on touch screens and from the keyboard.
 */
const props = defineProps<{
  help: FieldHelp
  /** What this field resolves to right now, e.g. "Auto uses VideoToolbox on this node". */
  note?: string
}>()

const open = ref(false)
const pinned = ref(false)
let timer: ReturnType<typeof setTimeout> | undefined

function hover(next: boolean) {
  clearTimeout(timer)
  if (pinned.value) return
  timer = setTimeout(() => (open.value = next), next ? 150 : 120)
}

function toggle() {
  clearTimeout(timer)
  pinned.value = !(open.value && pinned.value)
  open.value = pinned.value
}

function onOpenChange(next: boolean) {
  // Outside click or Escape.
  open.value = next
  if (!next) pinned.value = false
}
</script>

<template>
  <Popover :open="open" @update:open="onOpenChange">
    <PopoverAnchor as-child>
      <!-- A span, not a button: a disabled fieldset (a read-only preset)
           disables every button inside it, and help should still open. -->
      <span
        role="button"
        tabindex="0"
        class="cursor-help inline-grid place-items-center size-3.5 align-[-2px] rounded-full text-muted-foreground/50 hover:text-foreground focus-visible:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        :aria-label="`About ${props.help.title}`"
        :aria-expanded="open"
        @click.prevent.stop="toggle"
        @keydown.enter.prevent.stop="toggle"
        @keydown.space.prevent.stop="toggle"
        @pointerenter="(e) => e.pointerType === 'mouse' && hover(true)"
        @pointerleave="(e) => e.pointerType === 'mouse' && hover(false)"
      >
        <CircleHelp class="size-3" />
      </span>
    </PopoverAnchor>
    <PopoverContent
      side="top"
      align="start"
      class="w-80 gap-2 leading-relaxed"
      @open-auto-focus.prevent
      @pointerenter="hover(true)"
      @pointerleave="hover(false)"
    >
      <p class="font-semibold text-foreground">{{ props.help.title }}</p>
      <p class="text-muted-foreground">{{ props.help.body }}</p>
      <dl v-if="props.help.options?.length" class="grid grid-cols-[auto_1fr] gap-x-2.5 gap-y-1">
        <template v-for="o in props.help.options" :key="o.label">
          <dt class="font-medium text-foreground whitespace-nowrap">{{ o.label }}</dt>
          <dd class="text-muted-foreground">{{ o.text }}</dd>
        </template>
      </dl>
      <p v-if="props.note" class="rounded bg-muted px-2 py-1 text-foreground">{{ props.note }}</p>
      <p v-if="props.help.default" class="text-muted-foreground">
        <span class="font-medium text-foreground">Default:</span> {{ props.help.default }}
      </p>
    </PopoverContent>
  </Popover>
</template>
