<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { useStorage } from '@vueuse/core'
import { ArrowDownToLine, Trash2 } from '@lucide/vue'
import { useEventsStore, type LogEntry, type LogLevel } from '@/stores/events'
import { useNodesStore } from '@/stores/nodes'
import { Input } from '@/components/ui/input'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'

const events = useEventsStore()
const nodes = useNodesStore()

const LEVELS: { value: LogLevel; label: string }[] = [
  { value: 'error', label: 'Errors' },
  { value: 'warn', label: 'Warnings' },
  { value: 'rec', label: 'Recording' },
  { value: 'info', label: 'Info' },
]
const shown = useStorage<LogLevel[]>('cr.log.levels', ['error', 'warn', 'rec', 'info'])
const query = ref('')
const follow = ref(true)

const filtered = computed(() => {
  const q = query.value.trim().toLowerCase()
  return events.entries.filter(
    (e) =>
      shown.value.includes(e.level) &&
      (!q || `${e.message} ${e.detail ?? ''} ${e.node_id ? nodes.nameOf(e.node_id) : ''}`.toLowerCase().includes(q)),
  )
})

const levelClass: Record<LogLevel, string> = {
  error: 'text-destructive',
  warn: 'text-warning',
  rec: 'text-tally',
  info: 'text-info',
}

function time(e: LogEntry) {
  const t = e.at
  return `${t.toLocaleTimeString([], { hour12: false })}.${String(t.getMilliseconds()).padStart(3, '0')}`
}

// ── Follow tail ───────────────────────────────────────────────────────────────

const scroller = ref<HTMLElement | null>(null)

function scrollToEnd() {
  const el = scroller.value
  if (el) el.scrollTop = el.scrollHeight
}

watch(
  () => filtered.value.length,
  async () => {
    if (!follow.value) return
    await nextTick()
    scrollToEnd()
  },
  { immediate: true },
)

/** Scrolling up pauses following; scrolling back to the bottom resumes it. */
function onScroll() {
  const el = scroller.value
  if (el) follow.value = el.scrollHeight - el.scrollTop - el.clientHeight < 8
}

function resumeFollow() {
  follow.value = true
  scrollToEnd()
}

</script>

<template>
  <section class="h-full flex flex-col min-h-0">
    <div class="h-8 shrink-0 flex items-center gap-2 px-2 border-b border-border">
      <ToggleGroup
        v-model="shown"
        type="multiple"
        variant="segmented"
        size="sm"
      >
        <ToggleGroupItem
          v-for="l in LEVELS"
          :key="l.value"
          :value="l.value"
          class="h-6 px-2 text-[11px] gap-1"
        >
          <span class="size-1.5 rounded-full bg-current" :class="levelClass[l.value]" />
          {{ l.label }}
          <span class="num text-muted-foreground">{{ events.counts[l.value] }}</span>
        </ToggleGroupItem>
      </ToggleGroup>

      <Input v-model="query" placeholder="Filter…" class="h-6 w-48 text-xs" />

      <div class="flex-1" />
      <span class="num text-[11px] text-muted-foreground">{{ filtered.length }}/{{ events.entries.length }}</span>

      <Tooltip>
        <TooltipTrigger as-child>
          <button
            class="log-btn"
            :class="follow && 'text-info'"
            @click="resumeFollow"
          >
            <ArrowDownToLine class="size-3.5" />
          </button>
        </TooltipTrigger>
        <TooltipContent>{{ follow ? 'Following new entries' : 'Jump to latest and follow' }}</TooltipContent>
      </Tooltip>
      <Tooltip>
        <TooltipTrigger as-child>
          <button class="log-btn" @click="events.clear()"><Trash2 class="size-3.5" /></button>
        </TooltipTrigger>
        <TooltipContent>Clear log</TooltipContent>
      </Tooltip>
    </div>

    <div ref="scroller" class="flex-1 min-h-0 overflow-y-auto num text-[11px] leading-5 py-1" @scroll="onScroll">
      <p v-if="filtered.length === 0" class="px-3 py-2 font-sans text-xs text-muted-foreground">
        {{ events.entries.length ? 'No entries match the filter.' : 'Recording events, failures and node changes will appear here.' }}
      </p>
      <div
        v-for="e in filtered"
        :key="e.id"
        class="grid grid-cols-[auto_auto_auto_1fr] gap-x-3 px-3 hover:bg-accent/50"
      >
        <span class="text-muted-foreground">{{ time(e) }}</span>
        <span class="w-10 uppercase font-semibold" :class="levelClass[e.level]">{{ e.level }}</span>
        <span class="text-muted-foreground truncate max-w-40">{{ e.node_id ? nodes.nameOf(e.node_id) : '—' }}</span>
        <span class="min-w-0 break-words font-sans">
          {{ e.message }}
          <span v-if="e.detail" class="num text-muted-foreground ml-1">{{ e.detail }}</span>
        </span>
      </div>
    </div>
  </section>
</template>

<style scoped>
@reference "@/style.css";
.log-btn {
  @apply size-6 grid place-items-center rounded text-muted-foreground hover:text-foreground hover:bg-accent;
}
</style>
