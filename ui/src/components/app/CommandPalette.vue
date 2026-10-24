<script setup lang="ts">
import { computed, nextTick, ref, useTemplateRef, watch } from 'vue'
import { useEventListener } from '@vueuse/core'
import {
  ListboxContent,
  ListboxFilter,
  ListboxGroup,
  ListboxGroupLabel,
  ListboxItem,
  ListboxRoot,
} from 'reka-ui'
import { CornerDownLeft, History, Search, SearchX } from '@lucide/vue'
import {
  allCommands,
  GROUPS,
  noteCommandRun,
  recentCommandIds,
  scoreCommand,
  type Command,
} from '@/composables/useCommands'
import { shortcut } from '@/lib/keys'
import { useNodesStore } from '@/stores/nodes'
import { useSchedulesStore } from '@/stores/schedules'
import { Dialog, DialogContent, DialogDescription, DialogTitle } from '@/components/ui/dialog'
import { Kbd } from '@/components/ui/kbd'
import { useGlobalCommands } from './globalCommands'

const open = defineModel<boolean>('open', { required: true })

useGlobalCommands()

useEventListener('keydown', (e: KeyboardEvent) => {
  if (e.key.toLowerCase() === 'k' && (e.metaKey || e.ctrlKey)) {
    e.preventDefault()
    open.value = !open.value
  }
})

const query = ref('')
const listbox = useTemplateRef<{ highlightFirstItem: () => void }>('listbox')
const nodes = useNodesStore()
const schedules = useSchedulesStore()

// Schedules are otherwise loaded by their page; fetch them so they can be searched from anywhere.
watch(open, (v) => {
  if (!v) return
  query.value = ''
  if (nodes.isController && !schedules.loaded) schedules.load()
  // Enter runs the top item straight away, as it does after typing.
  nextTick(() => listbox.value?.highlightFirstItem())
})

/** Most results shown for a query; the list is for picking, not browsing. */
const MAX_RESULTS = 60

interface Section {
  heading: string
  recent?: boolean
  items: Command[]
}

const sections = computed<Section[]>(() => {
  if (!open.value) return []
  const commands = allCommands.value
  const q = query.value.trim()

  if (!q) {
    const byId = new Map(commands.map((c) => [c.id, c]))
    const recent = recentCommandIds.value.map((id) => byId.get(id)).filter((c): c is Command => !!c)
    const recentIds = new Set(recent.map((c) => c.id))
    const out: Section[] = recent.length ? [{ heading: 'Recent', recent: true, items: recent }] : []
    for (const g of GROUPS) {
      const items = commands.filter((c) => c.group === g && !c.searchOnly && !recentIds.has(c.id))
      if (items.length) out.push({ heading: g, items })
    }
    return out
  }

  // Rank everything, keep the best, then group them: groups ordered by their best match.
  const recentRank = new Map(recentCommandIds.value.map((id, i) => [id, i]))
  const ranked = commands
    .map((c) => {
      const s = scoreCommand(q, c)
      const r = recentRank.get(c.id)
      return { c, s: s && (r === undefined ? s : s + 60 - r * 5) }
    })
    .filter((x) => x.s > 0)
    .sort((a, b) => b.s - a.s)
    .slice(0, MAX_RESULTS)

  const groups = new Map<string, Command[]>()
  for (const { c } of ranked) {
    const list = groups.get(c.group)
    if (list) list.push(c)
    else groups.set(c.group, [c])
  }
  return [...groups].map(([heading, items]) => ({ heading, items }))
})

const empty = computed(() => sections.value.every((s) => !s.items.length))

/** Set when a command ran, so closing doesn't pull focus back over what it opened. */
let ran = false

function run(c: Command) {
  noteCommandRun(c.id)
  ran = true
  open.value = false
  void c.run()
}

function onCloseAutoFocus(e: Event) {
  if (ran) e.preventDefault()
  ran = false
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent
      class="top-[14vh] translate-y-0 sm:max-w-xl p-0 gap-0 overflow-hidden shadow-2xl"
      :show-close-button="false"
      @close-auto-focus="onCloseAutoFocus"
    >
      <DialogTitle class="sr-only">Command palette</DialogTitle>
      <DialogDescription class="sr-only">Search pages, sources, recordings and schedules, or run a command.</DialogDescription>

      <ListboxRoot ref="listbox" class="flex flex-col min-h-0" highlight-on-hover>
        <div class="flex items-center gap-2.5 px-3.5 h-12 border-b border-border">
          <Search class="size-4 shrink-0 text-muted-foreground" />
          <ListboxFilter
            v-model="query"
            auto-focus
            placeholder="Search pages, sources, recordings, commands…"
            class="flex-1 min-w-0 h-full bg-transparent text-sm outline-hidden placeholder:text-muted-foreground"
          />
          <Kbd>{{ shortcut('K') }}</Kbd>
        </div>

        <ListboxContent class="max-h-[min(60vh,26rem)] overflow-y-auto overscroll-contain scroll-py-2 p-1.5 outline-none">
          <div v-if="empty" class="py-10 px-6 text-center space-y-1.5">
            <SearchX class="size-6 mx-auto text-muted-foreground/70" />
            <p class="text-sm font-medium">No results for “{{ query.trim() }}”</p>
            <p class="text-xs text-muted-foreground">Try a page, a source, a recording or a schedule name.</p>
          </div>

          <ListboxGroup v-for="section in sections" :key="section.heading" class="not-first:mt-1.5">
            <ListboxGroupLabel class="flex items-center gap-1.5 px-2 pt-1.5 pb-1 text-[0.6875rem] font-medium uppercase tracking-wider text-muted-foreground">
              <History v-if="section.recent" class="size-3" />
              {{ section.heading }}
            </ListboxGroupLabel>
            <ListboxItem
              v-for="c in section.items"
              :key="c.id"
              :value="c.id"
              class="group/item relative flex h-8 cursor-default select-none items-center gap-2.5 rounded-md px-2 text-[0.8125rem] outline-hidden data-highlighted:bg-accent data-highlighted:text-accent-foreground"
              @select="run(c)"
            >
              <component
                :is="c.icon"
                v-if="c.icon"
                class="size-4 shrink-0 text-muted-foreground group-data-highlighted/item:text-foreground"
                :class="c.iconClass"
              />
              <span class="truncate">{{ c.title }}</span>
              <span v-if="c.detail" class="truncate text-xs text-muted-foreground">{{ c.detail }}</span>
              <span class="ml-auto flex shrink-0 items-center gap-2 pl-2">
                <Kbd v-if="c.shortcut">{{ c.shortcut }}</Kbd>
                <CornerDownLeft class="size-3.5 text-muted-foreground opacity-0 group-data-highlighted/item:opacity-100" />
              </span>
            </ListboxItem>
          </ListboxGroup>
        </ListboxContent>
      </ListboxRoot>

      <div class="flex items-center gap-4 h-9 px-3.5 border-t border-border bg-muted/40 text-[0.6875rem] text-muted-foreground">
        <span class="flex items-center gap-1.5"><Kbd class="key">↑</Kbd><Kbd class="key">↓</Kbd> navigate</span>
        <span class="flex items-center gap-1.5"><Kbd class="key">↵</Kbd> run</span>
        <span class="flex items-center gap-1.5"><Kbd class="key">esc</Kbd> close</span>
      </div>
    </DialogContent>
  </Dialog>
</template>

<style scoped>
@reference "@/style.css";
/* The footer sits on a muted strip, where the plain Kbd fill disappears. */
.key {
  @apply bg-background ring-1 ring-border;
}
</style>
