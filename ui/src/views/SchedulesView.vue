<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { CalendarClock, Pencil, Plus, Square } from '@lucide/vue'
import { sourceKey } from '@/composables/useApi'
import { registerCommands } from '@/composables/useCommands'
import { useSecondClock } from '@/composables/useSecondClock'
import { dateTime } from '@/lib/format'
import { notifyError } from '@/lib/notify'
import { relativeFuture, whatLabel, whenLabel } from '@/lib/schedules'
import { useNodesStore } from '@/stores/nodes'
import { useSchedulesStore } from '@/stores/schedules'
import { useSourcesStore } from '@/stores/sources'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import DataTable from '@/components/common/DataTable.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import StatusDot from '@/components/common/StatusDot.vue'
import ToolbarSearch from '@/components/common/ToolbarSearch.vue'
import ScheduleFormSheet from '@/components/schedules/ScheduleFormSheet.vue'
import type { Column } from '@/components/common/dataTable'
import type { ScheduleDto } from '@/types/generated/ScheduleDto'
import type { ScheduleInput } from '@/types/generated/ScheduleInput'
import type { ScheduleRunDto } from '@/types/generated/ScheduleRunDto'
import type { ScheduleRunStatus } from '@/types/generated/ScheduleRunStatus'

const store = useSchedulesStore()
const nodes = useNodesStore()
const sources = useSourcesStore()
const now = useSecondClock()

onMounted(store.load)
// A node made controller (or not) here changes what there is to show.
watch(() => nodes.isController, store.load)

const filter = ref('')

function sourceName(nodeId: string, id: string) {
  return sources.sources.find((s) => s.key === sourceKey(nodeId, id))?.display_name ?? id
}

const RUN: Record<ScheduleRunStatus, { label: string; dot: 'tally' | 'ok' | 'warn' | 'error' | 'off' }> = {
  running: { label: 'Running', dot: 'tally' },
  done: { label: 'Done', dot: 'ok' },
  partial: { label: 'Partly failed', dot: 'warn' },
  failed: { label: 'Failed', dot: 'error' },
  skipped: { label: 'Skipped', dot: 'off' },
}

const columns: Column<ScheduleDto>[] = [
  { id: 'name', label: 'Name', value: (s) => s.name, alwaysVisible: true, class: 'font-medium' },
  { id: 'when', label: 'When', value: (s) => whenLabel(s) },
  { id: 'what', label: 'What', value: (s) => whatLabel(s.actions, sourceName) },
  { id: 'next', label: 'Next', value: (s) => s.next_start ?? '' },
  { id: 'status', label: 'Last run', value: (s) => s.last_run?.status ?? '' },
  { id: 'enabled', label: 'Enabled', alwaysVisible: true, align: 'right' },
]

const selectedId = ref<string | null>(null)
const selected = computed(() => store.schedules.find((s) => s.id === selectedId.value) ?? null)
const runs = ref<ScheduleRunDto[]>([])

async function loadRuns() {
  const id = selectedId.value
  runs.value = id ? await store.runs(id).catch(() => []) : []
}
// Reload the history when the selection changes or its latest run does.
watch(
  () => [selectedId.value, selected.value?.last_run?.id, selected.value?.last_run?.status],
  loadRuns,
)

const editing = ref<ScheduleDto | null | 'new'>(null)

// From the command palette: `?select=<id>` picks a schedule, `?edit=<id>` opens it.
const route = useRoute()
const router = useRouter()
watch(
  () => [route.query.select, route.query.edit, store.loaded] as const,
  ([select, edit, loaded]) => {
    if (!loaded) return
    const id = typeof select === 'string' ? select : typeof edit === 'string' ? edit : null
    if (!id) return
    const s = store.schedules.find((x) => x.id === id)
    if (s) {
      selectedId.value = s.id
      if (edit !== undefined) editing.value = s
    }
    router.replace({ query: {} })
  },
  { immediate: true },
)

registerCommands(() =>
  nodes.isController
    ? [{ id: 'schedule:new', title: 'New schedule', group: 'This page', icon: Plus, keywords: ['add', 'create'], run: () => (editing.value = 'new') }]
    : [],
)

function inputOf(s: ScheduleDto): ScheduleInput {
  const { name, enabled, kind, date, weekdays, start_time, duration_s, actions } = s
  return { name, enabled, kind, date, weekdays, start_time, duration_s, actions }
}

async function setEnabled(s: ScheduleDto, on: boolean) {
  try {
    await store.update(s.id, { ...inputOf(s), enabled: on })
  } catch (e) {
    notifyError(`Couldn’t ${on ? 'enable' : 'disable'} “${s.name}”`, e)
  }
}

async function stop(s: ScheduleDto) {
  try {
    await store.stop(s.id)
  } catch (e) {
    notifyError(`Couldn’t stop “${s.name}”`, e)
  }
}
</script>

<template>
  <div class="h-full flex flex-col min-h-0">
    <PageHeader title="Schedule" :count="nodes.isController ? store.schedules.length : undefined">
      <template v-if="nodes.isController" #actions>
        <Button size="sm" @click="editing = 'new'"><Plus /> New schedule</Button>
      </template>
      <ToolbarSearch v-if="nodes.isController" v-model="filter" />
    </PageHeader>

    <div v-if="!nodes.isController" class="flex-1 grid place-items-center p-8">
      <div class="max-w-sm text-center space-y-2">
        <CalendarClock class="size-8 mx-auto text-muted-foreground" />
        <p class="text-sm font-medium">Schedules run on the controller</p>
        <p class="text-xs text-muted-foreground">
          Open the controller’s page to schedule recordings and playout, or make this node the controller under Setup › Nodes.
        </p>
      </div>
    </div>

    <template v-else>
      <div class="flex-1 min-h-0 overflow-y-auto p-4">
        <DataTable
          table-id="schedules"
          :columns="columns"
          :rows="store.schedules"
          :row-key="(s) => s.id"
          :filter="filter"
          :row-class="(s) => (s.id === selectedId ? 'bg-accent/60 cursor-pointer' : 'cursor-pointer')"
          @row-click="(s) => (selectedId = selectedId === s.id ? null : s.id)"
        >
          <template #cell-when="{ row }">
            <span class="num">{{ whenLabel(row) }}</span>
          </template>
          <template #cell-what="{ row }">
            <span class="text-muted-foreground">{{ whatLabel(row.actions, sourceName) }}</span>
          </template>
          <template #cell-next="{ row }">
            <span v-if="row.next_start" :title="dateTime(row.next_start)">{{ relativeFuture(row.next_start, now) }}</span>
            <span v-else class="text-muted-foreground">—</span>
          </template>
          <template #cell-status="{ row }">
            <span v-if="row.last_run" class="flex items-center gap-1.5" :class="row.last_run.status === 'running' && 'text-tally font-medium'">
              <StatusDot :status="RUN[row.last_run.status].dot" />
              {{ RUN[row.last_run.status].label }}
              <Button
                v-if="row.last_run.status === 'running'"
                variant="outline"
                size="sm"
                class="h-6 ml-1"
                @click.stop="stop(row)"
              >
                <Square /> Stop now
              </Button>
            </span>
            <span v-else class="text-muted-foreground">Not run yet</span>
          </template>
          <template #cell-enabled="{ row }">
            <span class="inline-flex" @click.stop>
              <Switch :model-value="row.enabled" @update:model-value="(v: boolean) => setEnabled(row, v)" />
            </span>
          </template>
          <template #empty>
            <div class="py-8 text-center space-y-2">
              <p class="text-sm text-muted-foreground">No schedules yet.</p>
              <Button size="sm" variant="outline" @click="editing = 'new'"><Plus /> New schedule</Button>
            </div>
          </template>
        </DataTable>
      </div>

      <!-- The selected schedule's recent runs -->
      <section v-if="selected" class="shrink-0 max-h-[40%] overflow-y-auto border-t border-border bg-card px-4 py-3 space-y-2">
        <div class="flex items-center gap-3">
          <h2 class="text-sm font-semibold">{{ selected.name }}</h2>
          <span class="num text-xs text-muted-foreground">{{ whenLabel(selected) }}</span>
          <div class="flex-1" />
          <Button variant="outline" size="sm" @click="editing = selected"><Pencil /> Edit</Button>
        </div>
        <p v-if="!runs.length" class="text-xs text-muted-foreground">It hasn’t run yet.</p>
        <div v-for="run in runs" :key="run.id" class="text-xs space-y-0.5">
          <div class="flex items-center gap-2">
            <StatusDot :status="RUN[run.status].dot" />
            <span class="num">{{ dateTime(run.planned_start) }}</span>
            <span class="font-medium">{{ RUN[run.status].label }}</span>
            <span v-if="run.stopped_at" class="text-muted-foreground">ended {{ dateTime(run.stopped_at) }}</span>
          </div>
          <p
            v-for="t in run.targets.filter((t) => t.error)"
            :key="`${t.node_id}/${t.id}`"
            class="pl-4 text-destructive"
          >
            {{ sourceName(t.node_id, t.id) }}{{ nodes.nodes.length > 1 ? ` (${nodes.nameOf(t.node_id)})` : '' }}: {{ t.error }}
          </p>
        </div>
      </section>
    </template>

    <ScheduleFormSheet
      v-if="editing"
      :schedule="editing === 'new' ? null : editing"
      @close="editing = null"
    />
  </div>
</template>
