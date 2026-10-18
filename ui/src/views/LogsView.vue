<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { useStorage, useVirtualList } from '@vueuse/core'
import { ArrowDownToLine, History, Trash2, X } from '@lucide/vue'
import { clockTimeFine, dateTime } from '@/lib/format'
import { useEventsStore } from '@/stores/events'
import { useLogsStore, type LogRow, type RowLevel, type RowSource } from '@/stores/logs'
import { useNodesStore } from '@/stores/nodes'
import { Button } from '@/components/ui/button'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import CopyButton from '@/components/common/CopyButton.vue'
import OptionSelect from '@/components/OptionSelect.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import ToolbarSearch from '@/components/common/ToolbarSearch.vue'

const logs = useLogsStore()
const events = useEventsStore()
const nodes = useNodesStore()

onMounted(logs.open)
onUnmounted(logs.close)

// Errors count as seen while the log is on screen.
watch(
  () => events.unseenErrors,
  (n) => n && events.markSeen(),
  { immediate: true },
)

const LEVELS: { value: RowLevel; label: string }[] = [
  { value: 'error', label: 'Errors' },
  { value: 'warn', label: 'Warnings' },
  { value: 'rec', label: 'Recording' },
  { value: 'info', label: 'Info' },
  { value: 'debug', label: 'Debug' },
]
const shown = useStorage<RowLevel[]>('cr.logs.levels', ['error', 'warn', 'rec', 'info'])
// Debug lines are only fetched and streamed while shown: there are many.
watch(
  () => shown.value.includes('debug'),
  (debug) => (logs.level = debug ? 'debug' : 'info'),
  { immediate: true },
)

const SOURCES: { value: 'all' | RowSource; label: string }[] = [
  { value: 'all', label: 'All' },
  { value: 'node', label: 'Node logs' },
  { value: 'activity', label: 'Activity' },
]
const source = useStorage<'all' | RowSource>('cr.logs.source', 'all')

const nodeFilter = ref('all')
const nodeOptions = computed(() => [
  { value: 'all', label: 'All nodes' },
  ...nodes.nodes.map((n) => ({ value: n.id, label: nodes.labelOf(n.id) })),
])

const query = ref('')

const filtered = computed(() => {
  const q = query.value.trim().toLowerCase()
  const levels = new Set(shown.value)
  // Trace lines only come with debug shown.
  if (levels.has('debug')) levels.add('trace')
  return logs.rows.filter(
    (r) =>
      levels.has(r.level) &&
      (source.value === 'all' || r.source === source.value) &&
      (nodeFilter.value === 'all' || r.node_id === nodeFilter.value) &&
      (!q ||
        `${r.message} ${r.detail} ${r.target ?? ''} ${r.node_id ? nodes.nameOf(r.node_id) : ''}`
          .toLowerCase()
          .includes(q)),
  )
})

const counts = computed(() => {
  const c: Record<RowLevel, number> = { error: 0, warn: 0, rec: 0, info: 0, debug: 0, trace: 0 }
  for (const r of logs.rows) c[r.level]++
  c.debug += c.trace
  return c
})

const levelClass: Record<RowLevel, string> = {
  error: 'text-destructive',
  warn: 'text-warning',
  rec: 'text-tally',
  info: 'text-info',
  debug: 'text-muted-foreground',
  trace: 'text-muted-foreground',
}

const ROW_HEIGHT = 20
const { list, containerProps, wrapperProps, scrollTo } = useVirtualList(filtered, {
  itemHeight: ROW_HEIGHT,
  overscan: 30,
})

const follow = ref(true)

function scrollToEnd() {
  if (filtered.value.length) scrollTo(filtered.value.length - 1)
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
  containerProps.onScroll()
  const el = containerProps.ref.value
  if (el) follow.value = el.scrollHeight - el.scrollTop - el.clientHeight < ROW_HEIGHT
}

function resumeFollow() {
  follow.value = true
  scrollToEnd()
}

const selected = ref<LogRow | null>(null)

/** The node whose older lines "Load older" fetches: the filtered one, else all. */
const olderNode = computed(() => (nodeFilter.value === 'all' ? null : nodeFilter.value))
const hasOlder = computed(() =>
  (olderNode.value ? [olderNode.value] : nodes.reachable.map((n) => n.id)).some((id) => !logs.exhausted.has(id)),
)

async function loadOlder() {
  const before = filtered.value.length
  follow.value = false
  await logs.loadOlder(olderNode.value)
  await nextTick()
  // Keep the line that was at the top in view.
  scrollTo(filtered.value.length - before)
}

/** Log files to point at for history beyond memory: the filtered node's, else every node's. */
const files = computed(() =>
  [...logs.logFiles.entries()]
    .filter(([id, path]) => path && (nodeFilter.value === 'all' || id === nodeFilter.value))
    .map(([id, path]) => ({ id, path: path! })),
)
</script>

<template>
  <div class="h-full flex flex-col min-h-0">
    <PageHeader title="Logs" :count="filtered.length">
      <template #actions>
        <Button v-if="hasOlder" variant="outline" size="sm" :disabled="logs.loadingOlder" @click="loadOlder">
          <History /> Load older
        </Button>
      </template>

      <ToolbarSearch v-model="query" />
      <ToggleGroup v-model="shown" type="multiple" variant="segmented">
        <ToggleGroupItem v-for="l in LEVELS" :key="l.value" :value="l.value" class="px-2 gap-1">
          <span class="size-1.5 rounded-full bg-current" :class="levelClass[l.value]" />
          {{ l.label }}
          <span class="num text-muted-foreground">{{ counts[l.value] }}</span>
        </ToggleGroupItem>
      </ToggleGroup>
      <ToggleGroup
        :model-value="source"
        type="single"
        variant="segmented"
        @update:model-value="(v) => v && (source = v as typeof source)"
      >
        <ToggleGroupItem v-for="s in SOURCES" :key="s.value" :value="s.value" class="px-2.5">{{ s.label }}</ToggleGroupItem>
      </ToggleGroup>
      <OptionSelect v-if="nodes.nodes.length > 1" v-model="nodeFilter" :options="nodeOptions" class="w-40 text-xs" />

      <template #view>
        <Tooltip>
          <TooltipTrigger as-child>
            <button class="log-btn" :class="follow && 'text-info'" @click="resumeFollow">
              <ArrowDownToLine class="size-3.5" />
            </button>
          </TooltipTrigger>
          <TooltipContent>{{ follow ? 'Following new lines' : 'Jump to latest and follow' }}</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger as-child>
            <button class="log-btn" @click="logs.clear(); selected = null"><Trash2 class="size-3.5" /></button>
          </TooltipTrigger>
          <TooltipContent>Clear</TooltipContent>
        </Tooltip>
      </template>
    </PageHeader>

    <div
      :ref="containerProps.ref"
      :style="containerProps.style"
      class="flex-1 min-h-0 num text-[11px] py-1"
      @scroll="onScroll"
    >
      <p v-if="filtered.length === 0" class="px-4 py-2 font-sans text-xs text-muted-foreground">
        {{ logs.rows.length ? 'No lines match the filter.' : 'Node log lines, recording events, failures and node changes will appear here.' }}
      </p>
      <div v-bind="wrapperProps">
        <div
          v-for="{ data: r } in list"
          :key="r.key"
          class="grid grid-cols-[auto_2.5rem_8rem_1fr] gap-x-3 px-4 cursor-default hover:bg-accent/50"
          :class="selected?.key === r.key && 'bg-accent'"
          :style="{ height: `${ROW_HEIGHT}px`, lineHeight: `${ROW_HEIGHT}px` }"
          @click="selected = selected?.key === r.key ? null : r"
        >
          <span class="text-muted-foreground">{{ clockTimeFine(r.at, 3) }}</span>
          <span class="uppercase font-semibold" :class="levelClass[r.level]">{{ r.level }}</span>
          <span class="text-muted-foreground truncate">{{ r.node_id ? nodes.nameOf(r.node_id) : '—' }}</span>
          <span class="min-w-0 truncate" :class="r.source === 'activity' && 'font-sans'">
            {{ r.message }}
            <span v-if="r.detail" class="text-muted-foreground ml-1">{{ r.detail }}</span>
          </span>
        </div>
      </div>
    </div>

    <!-- The selected line in full -->
    <section v-if="selected" class="shrink-0 max-h-48 overflow-y-auto border-t border-border bg-card px-4 py-2 text-xs space-y-1">
      <div class="flex items-center gap-3 text-muted-foreground">
        <span class="num">{{ dateTime(selected.at) }}</span>
        <span class="uppercase font-semibold" :class="levelClass[selected.level]">{{ selected.level }}</span>
        <span v-if="selected.node_id">{{ nodes.labelOf(selected.node_id) }}</span>
        <span v-if="selected.target" class="num">{{ selected.target }}</span>
        <span v-else>Activity</span>
        <div class="flex-1" />
        <CopyButton :value="`${selected.message} ${selected.detail}`.trim()" label="Copy line" />
        <button class="log-btn" aria-label="Close" @click="selected = null"><X class="size-3.5" /></button>
      </div>
      <p class="whitespace-pre-wrap break-words">{{ selected.message }}</p>
      <p v-if="selected.detail" class="num whitespace-pre-wrap break-words text-muted-foreground">{{ selected.detail }}</p>
    </section>

    <footer v-if="files.length" class="shrink-0 border-t border-border px-4 py-1.5 text-[11px] text-muted-foreground space-y-0.5">
      <p v-for="f in files" :key="f.id" class="flex items-center gap-1.5 min-w-0">
        <span class="shrink-0">Every line{{ nodes.nodes.length > 1 ? ` from ${nodes.nameOf(f.id)}` : '' }}, including older ones, is in</span>
        <span class="num truncate text-foreground">{{ f.path }}</span>
        <CopyButton :value="f.path" label="Copy path" />
      </p>
    </footer>
  </div>
</template>

<style scoped>
@reference "@/style.css";
.log-btn {
  @apply size-6 grid place-items-center rounded text-muted-foreground hover:text-foreground hover:bg-accent;
}
</style>
