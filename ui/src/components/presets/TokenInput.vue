<script setup lang="ts">
import { computed, nextTick, ref } from 'vue'
import { Braces } from '@lucide/vue'
import type { PathToken } from '@/lib/codecs'
import { unknownToken } from '@/lib/codecs'
import { Popover, PopoverAnchor, PopoverContent } from '@/components/ui/popover'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'

/**
 * A text field for path templates. Tokens are coloured as you type (unknown
 * ones underlined in red), typing `{` suggests tokens, and the `{}` button
 * lists them all. The value stays plain text, so copy and paste just work.
 */
const props = defineProps<{
  groups: readonly { label: string; tokens: readonly PathToken[] }[]
  invalid?: boolean
  placeholder?: string
  ariaLabel?: string
  /** Fixed text after the value, e.g. `.{ext}`. */
  suffix?: string
}>()
const model = defineModel<string>({ required: true })

const input = ref<HTMLInputElement | null>(null)
const mirror = ref<HTMLDivElement | null>(null)

// ── Highlighting ──────────────────────────────────────────────────────────────

const parts = computed(() =>
  model.value
    .split(/(\{[^{}]*\}|\/)/)
    .filter(Boolean)
    .map((text) => ({
      text,
      kind: text === '/' ? 'sep' : text.startsWith('{') && text.endsWith('}') ? (unknownToken(text) ? 'bad' : 'token') : 'text',
    })),
)

function syncScroll() {
  if (mirror.value && input.value) mirror.value.scrollLeft = input.value.scrollLeft
}

// ── Inserting ─────────────────────────────────────────────────────────────────

/** The selection when the field last had focus, so the menu inserts where the cursor was. */
let selection: [number, number] | null = null

function remember() {
  const el = input.value
  if (el) selection = [el.selectionStart ?? el.value.length, el.selectionEnd ?? el.value.length]
}

async function replaceRange(start: number, end: number, token: string) {
  model.value = model.value.slice(0, start) + token + model.value.slice(end)
  await nextTick()
  const el = input.value
  el?.focus()
  el?.setSelectionRange(start + token.length, start + token.length)
  syncScroll()
}

function insert(token: string) {
  const [start, end] = selection ?? [model.value.length, model.value.length]
  replaceRange(start, end, token)
}

// ── Suggestions after `{` ─────────────────────────────────────────────────────

const allTokens = computed(() => props.groups.flatMap((g) => g.tokens))
const query = ref<{ start: number; text: string } | null>(null)
const active = ref(0)

const suggestions = computed(() => {
  if (!query.value) return []
  const q = query.value.text.toLowerCase()
  const name = (t: PathToken) => t.token.slice(1, -1)
  return [
    ...allTokens.value.filter((t) => name(t).startsWith(q)),
    ...allTokens.value.filter((t) => !name(t).startsWith(q) && name(t).includes(q)),
  ]
})
const suggesting = computed(() => suggestions.value.length > 0)

/** Look behind the cursor for an unfinished `{name`. */
function updateQuery() {
  remember()
  syncScroll()
  const el = input.value
  if (!el || el.selectionStart !== el.selectionEnd || document.activeElement !== el) {
    query.value = null
    return
  }
  const before = el.value.slice(0, el.selectionStart ?? 0)
  const m = before.match(/\{([a-z_]*)$/i)
  const next = m ? { start: before.length - m[0].length, text: m[1]! } : null
  if (next?.text !== query.value?.text || next?.start !== query.value?.start) active.value = 0
  query.value = next
}

function accept(t: PathToken) {
  const el = input.value
  if (!query.value || !el) return
  const caret = el.selectionStart ?? el.value.length
  // Also replace the rest of a token being edited in the middle: "{da|te}".
  const rest = el.value.slice(caret).match(/^[a-z_]*\}?/i)![0]
  const start = query.value.start
  query.value = null
  replaceRange(start, caret + rest.length, t.token)
}

function onKeydown(e: KeyboardEvent) {
  if (!suggesting.value) return
  const n = suggestions.value.length
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    active.value = (active.value + (e.key === 'ArrowDown' ? 1 : n - 1)) % n
  } else if (e.key === 'Enter' || e.key === 'Tab') {
    accept(suggestions.value[active.value]!)
  } else if (e.key === 'Escape') {
    query.value = null
    // Don't also close the sheet the field is in.
    e.stopPropagation()
  } else return
  e.preventDefault()
}

function onInteractOutside(e: Event) {
  if (input.value && e.target instanceof Node && input.value.contains(e.target)) e.preventDefault()
  else query.value = null
}
</script>

<template>
  <Popover :open="suggesting">
    <PopoverAnchor as-child>
      <div
        class="flex h-7 w-full items-center rounded-md border border-input bg-transparent shadow-sm transition-colors focus-within:ring-1 focus-within:ring-ring"
        :class="invalid && 'border-destructive ring-destructive/20'"
      >
        <div class="relative flex-1 min-w-0 h-full">
          <div
            ref="mirror"
            aria-hidden="true"
            class="absolute inset-0 px-2.5 flex items-center overflow-hidden whitespace-pre text-xs pointer-events-none"
          >
            <span v-if="!model && placeholder" class="text-muted-foreground">{{ placeholder }}</span>
            <span
              v-for="(p, i) in parts"
              :key="i"
              :class="{
                'text-primary': p.kind === 'token',
                'text-destructive underline decoration-wavy underline-offset-2': p.kind === 'bad',
                'text-muted-foreground': p.kind === 'sep',
              }"
              >{{ p.text }}</span
            ><span> </span>
          </div>
          <input
            ref="input"
            v-model="model"
            :aria-label="ariaLabel"
            :aria-invalid="invalid"
            spellcheck="false"
            autocomplete="off"
            class="relative h-full w-full bg-transparent px-2.5 text-xs text-transparent caret-foreground outline-none selection:bg-primary/25"
            @input="updateQuery"
            @click="updateQuery"
            @keyup="updateQuery"
            @keydown="onKeydown"
            @scroll="syncScroll"
            @blur="query = null"
          />
        </div>
        <span v-if="suffix" class="pl-0.5 pr-1.5 text-xs text-muted-foreground select-none">{{ suffix }}</span>
        <DropdownMenu>
          <Tooltip>
            <TooltipTrigger as-child>
              <DropdownMenuTrigger as-child>
                <button
                  type="button"
                  class="h-full px-1.5 border-l border-input text-muted-foreground hover:text-primary rounded-r-md"
                  aria-label="Insert token"
                >
                  <Braces class="size-3.5" />
                </button>
              </DropdownMenuTrigger>
            </TooltipTrigger>
            <TooltipContent>Insert token (or type {)</TooltipContent>
          </Tooltip>
          <DropdownMenuContent align="end" class="w-72 max-h-80" @close-auto-focus.prevent>
            <DropdownMenuGroup v-for="g in groups" :key="g.label">
              <DropdownMenuLabel class="text-[10px] uppercase tracking-wider text-muted-foreground/70">{{ g.label }}</DropdownMenuLabel>
              <DropdownMenuItem v-for="t in g.tokens" :key="t.token" class="items-baseline gap-2" @select="insert(t.token)">
                <span class="num text-primary shrink-0">{{ t.token }}</span>
                <span class="text-muted-foreground text-[11px] truncate">{{ t.help }}</span>
              </DropdownMenuItem>
            </DropdownMenuGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </PopoverAnchor>
    <PopoverContent
      align="start"
      class="w-72 p-1 gap-0"
      @open-auto-focus.prevent
      @close-auto-focus.prevent
      @interact-outside="onInteractOutside"
      @escape-key-down.prevent
    >
      <button
        v-for="(t, i) in suggestions"
        :key="t.token"
        type="button"
        class="flex items-baseline gap-2 rounded px-2 py-1 text-left"
        :class="i === active && 'bg-accent'"
        @mousedown.prevent="accept(t)"
        @mouseenter="active = i"
      >
        <span class="num text-primary shrink-0">{{ t.token }}</span>
        <span class="text-muted-foreground text-[11px] truncate">{{ t.help }}</span>
      </button>
    </PopoverContent>
  </Popover>
</template>
