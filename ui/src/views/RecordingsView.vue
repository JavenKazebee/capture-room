<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { useEventListener, useNow, useStorage } from '@vueuse/core'
import { Play } from '@lucide/vue'
import { clockTime, dayKey, dayLabel, formatDuration } from '@/lib/format'
import { CODECS } from '@/lib/codecs'
import { notifyError } from '@/lib/notify'
import { useNodesStore } from '@/stores/nodes'
import { previewOutput, totalDropped, useRecordingsStore, type RecordingSession } from '@/stores/recordings'
import { Button } from '@/components/ui/button'
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from '@/components/ui/resizable'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import OptionSelect from '@/components/OptionSelect.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import ToolbarSearch from '@/components/common/ToolbarSearch.vue'
import DataTable from '@/components/common/DataTable.vue'
import ColumnsMenu from '@/components/common/ColumnsMenu.vue'
import type { Column } from '@/components/common/dataTable'
import StatusDot from '@/components/common/StatusDot.vue'
import RecordingDetail from '@/components/recordings/RecordingDetail.vue'

const recordings = useRecordingsStore()
const nodes = useNodesStore()
const now = useNow({ interval: 1000 })

const filter = ref('')
type StatusFilter = 'all' | 'active' | 'stopped' | 'error'
const statusFilter = useStorage<StatusFilter>('cr.recordings.status', 'all')
const nodeFilter = ref('all')

const STATUS_OPTIONS: { value: StatusFilter; label: string }[] = [
  { value: 'all', label: 'All' },
  { value: 'active', label: 'Recording' },
  { value: 'stopped', label: 'Finished' },
  { value: 'error', label: 'Failed' },
]

const SHORTCUTS: [string[], string][] = [
  [['↑', '↓'], 'Select the previous or next recording'],
  [['Space'], 'Play or pause'],
  [['Esc'], 'Close the recording'],
]

const nodeOptions = computed(() => [
  { value: 'all', label: 'All nodes' },
  ...nodes.nodes.map((n) => ({ value: n.id, label: nodes.labelOf(n.id) })),
])

const rows = computed(() =>
  recordings.history.filter(
    (s) =>
      (statusFilter.value === 'all' || s.status === statusFilter.value) &&
      (nodeFilter.value === 'all' || s.node_id === nodeFilter.value),
  ),
)

const key = (s: RecordingSession) => `${s.node_id}/${s.id}`

// ── Selection ─────────────────────────────────────────────────────────────────

const selectedKey = ref<string | null>(null)
const selected = computed(() => recordings.sessions.find((s) => key(s) === selectedKey.value) ?? null)

// A session removed (here or from another client) closes its detail.
watch(selected, (s) => {
  if (!s) selectedKey.value = null
})

function select(s: RecordingSession) {
  selectedKey.value = selectedKey.value === key(s) ? null : key(s)
}

const table = ref<{ displayedRows: RecordingSession[] }>()
const detail = ref<InstanceType<typeof RecordingDetail>>()

/** Moves the selection up or down the table as shown, keeping the row in view. */
async function step(by: 1 | -1) {
  const shown = table.value?.displayedRows ?? []
  if (!shown.length) return
  const i = shown.findIndex((s) => key(s) === selectedKey.value)
  const next = shown[i === -1 ? (by === 1 ? 0 : shown.length - 1) : Math.min(Math.max(i + by, 0), shown.length - 1)]!
  selectedKey.value = key(next)
  await nextTick()
  document.querySelector(`[data-row-key="${CSS.escape(key(next))}"]`)?.scrollIntoView({ block: 'nearest' })
}

useEventListener('keydown', (e: KeyboardEvent) => {
  const t = e.target as HTMLElement
  if (e.metaKey || e.ctrlKey || e.altKey) return
  if (t.closest('input, textarea, select, video, button, a, [role="button"], [role="dialog"], [role="listbox"], [role="menu"]')) return
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    e.preventDefault()
    void step(e.key === 'ArrowDown' ? 1 : -1)
  } else if (e.key === ' ' && detail.value) {
    e.preventDefault()
    detail.value.togglePlay()
  } else if (e.key === 'Escape') selectedKey.value = null
})

// ── Columns ───────────────────────────────────────────────────────────────────

const STATUS = {
  active: { label: 'Recording', dot: 'tally', order: 0 },
  error: { label: 'Failed', dot: 'error', order: 1 },
  stopped: { label: 'Finished', dot: 'off', order: 2 },
} as const

function durationMs(s: RecordingSession) {
  const end = s.stopped_at ? new Date(s.stopped_at).getTime() : now.value.getTime()
  return end - new Date(s.started_at).getTime()
}


/** The codecs a session recorded, e.g. "ProRes 422 HQ + H.264". */
function formats(s: RecordingSession) {
  if (!s.outputs.length) return `${s.output_paths.length} output${s.output_paths.length === 1 ? '' : 's'}`
  return s.outputs.map((o) => CODECS[o.codec]).join(' + ')
}

const columns = computed<Column<RecordingSession>[]>(() => [
  { id: 'status', label: 'Status', value: (s) => STATUS[s.status].order, class: 'w-px whitespace-nowrap' },
  { id: 'source', label: 'Source', value: (s) => recordings.sourceNameOf(s), alwaysVisible: true, class: 'min-w-36' },
  { id: 'node', label: 'Node', value: (s) => nodes.nameOf(s.node_id), hiddenByDefault: nodes.nodes.length < 2 },
  { id: 'preset', label: 'Preset', value: (s) => recordings.presetNameOf(s) },
  { id: 'start', label: 'Start', value: (s) => s.started_at, class: 'num whitespace-nowrap' },
  { id: 'end', label: 'End', value: (s) => s.stopped_at, class: 'num whitespace-nowrap' },
  { id: 'duration', label: 'Duration', value: (s) => durationMs(s), align: 'right', class: 'num whitespace-nowrap' },
  { id: 'formats', label: 'Outputs', value: (s) => formats(s) },
  { id: 'dropped', label: 'Dropped', value: (s) => totalDropped(s), align: 'right', hiddenByDefault: true, class: 'num' },
  { id: 'id', label: 'Session ID', value: (s) => s.id, hiddenByDefault: true, class: 'num' },
])

// ── Load older ────────────────────────────────────────────────────────────────

async function loadOlder() {
  try {
    await recordings.loadOlder()
  } catch (e) {
    notifyError('Could not load older recordings', e)
  }
}
</script>

<template>
  <div class="h-full flex flex-col min-h-0">
    <PageHeader title="Recordings" :count="rows.length" :shortcuts="SHORTCUTS">
      <ToolbarSearch v-model="filter" />
      <ToggleGroup
        :model-value="statusFilter"
        type="single"
        variant="segmented"
        @update:model-value="(v) => v && (statusFilter = v as StatusFilter)"
      >
        <ToggleGroupItem v-for="o in STATUS_OPTIONS" :key="o.value" :value="o.value" class="px-2.5">{{ o.label }}</ToggleGroupItem>
      </ToggleGroup>
      <OptionSelect v-if="nodes.nodes.length > 1" v-model="nodeFilter" :options="nodeOptions" class="w-40 text-xs" />

      <template #view>
        <ColumnsMenu table-id="recordings" :columns="columns" />
      </template>
    </PageHeader>

    <ResizablePanelGroup direction="vertical" auto-save-id="cr.recordings.layout" class="flex-1 min-h-0">
      <ResizablePanel id="table" :order="1" :min-size="20">
        <div class="h-full overflow-y-auto p-4 space-y-3">
          <DataTable
            ref="table"
            table-id="recordings"
            :columns="columns"
            :rows="rows"
            :row-key="key"
            :filter="filter"
            :group-by="(s) => dayKey(s.started_at)"
            :group-label="(k) => dayLabel(k, now)"
            :row-class="(s) => (key(s) === selectedKey ? 'bg-accent/60 cursor-pointer' : 'cursor-pointer')"
            @row-click="select"
          >
            <template #cell-status="{ row }">
              <span class="flex items-center gap-1.5" :class="row.status === 'active' && 'text-tally font-medium'">
                <StatusDot :status="STATUS[row.status].dot" /> {{ STATUS[row.status].label }}
              </span>
            </template>
            <template #cell-source="{ row }">
              <span class="flex items-center gap-1.5 font-medium text-sm">
                {{ recordings.sourceNameOf(row) }}
                <Play
                  v-if="row.status !== 'active' && previewOutput(row) !== -1"
                  class="size-3 text-muted-foreground"
                  aria-label="Can be previewed"
                />
              </span>
              <div v-if="row.error_message" class="text-destructive truncate max-w-96" :title="row.error_message">
                {{ row.error_message }}
              </div>
            </template>
            <template #cell-node="{ row }">{{ nodes.nameOf(row.node_id) }}</template>
            <template #cell-preset="{ row }"><span class="text-muted-foreground">{{ recordings.presetNameOf(row) }}</span></template>
            <template #cell-start="{ row }"><span class="text-muted-foreground">{{ clockTime(row.started_at) }}</span></template>
            <template #cell-end="{ row }">
              <span v-if="row.stopped_at" class="text-muted-foreground">{{ clockTime(row.stopped_at) }}</span>
              <span v-else class="text-tally font-medium">now</span>
            </template>
            <template #cell-duration="{ row }">
              <span :class="row.status === 'active' && 'text-tally font-medium'">{{ formatDuration(durationMs(row)) }}</span>
            </template>
            <template #cell-formats="{ row }"><span class="text-muted-foreground">{{ formats(row) }}</span></template>
            <template #cell-dropped="{ row }">
              <span :class="totalDropped(row) > 0 && 'text-warning'">{{ totalDropped(row).toLocaleString() }}</span>
            </template>
            <template #empty>
              <template v-if="filter || statusFilter !== 'all' || nodeFilter !== 'all'">No recordings match the filters.</template>
              <template v-else>No recordings yet. Sessions from every node appear here once they start.</template>
            </template>
          </DataTable>

          <div v-if="recordings.hasOlder" class="flex justify-center">
            <Button variant="outline" size="sm" class="h-7 text-xs" :disabled="recordings.loadingOlder" @click="loadOlder">
              {{ recordings.loadingOlder ? 'Loading…' : 'Load older recordings' }}
            </Button>
          </div>
        </div>
      </ResizablePanel>

      <template v-if="selected">
        <ResizableHandle />
        <ResizablePanel id="detail" :order="2" :default-size="45" :min-size="25" :max-size="75">
          <RecordingDetail ref="detail" :key="key(selected)" :session="selected" @close="selectedKey = null" />
        </ResizablePanel>
      </template>
    </ResizablePanelGroup>
  </div>
</template>
