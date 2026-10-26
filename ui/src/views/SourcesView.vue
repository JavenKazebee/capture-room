<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { toast } from 'vue-sonner'
import { ListChecks, MonitorPlay, Pencil, Plus, Radio, RadioTower, RefreshCw, Trash2, X } from '@lucide/vue'
import { CONFIGURED_TYPES, useSourcesStore, type Source } from '@/stores/sources'
import { useNodesStore } from '@/stores/nodes'
import { useRecordingsStore } from '@/stores/recordings'
import { useRecordDeskStore } from '@/stores/recordDesk'
import { notifyError } from '@/lib/notify'
import { registerCommands } from '@/composables/useCommands'
import { reloadAll } from '@/composables/useWebSocket'
import { useSelection } from '@/composables/useSelection'
import { BulkSkip, bulkToast, runBulk } from '@/lib/bulk'
import { plural } from '@/lib/format'
import { fpsLabel, resolutionLabel } from '@/lib/sourceFormat'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Switch } from '@/components/ui/switch'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import PageHeader from '@/components/common/PageHeader.vue'
import ToolbarField from '@/components/common/ToolbarField.vue'
import ToolbarSearch from '@/components/common/ToolbarSearch.vue'
import DataTable from '@/components/common/DataTable.vue'
import ColumnsMenu from '@/components/common/ColumnsMenu.vue'
import BulkActionBar from '@/components/common/BulkActionBar.vue'
import type { Column } from '@/components/common/dataTable'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import CopyButton from '@/components/common/CopyButton.vue'
import StatusDot from '@/components/common/StatusDot.vue'
import LoadError from '@/components/common/LoadError.vue'
import SourceFormSheet from '@/components/sources/SourceFormSheet.vue'
import NdiSendersSheet from '@/components/sources/NdiSendersSheet.vue'
import { LINK } from '@/lib/linkState'
import { useStorage } from '@vueuse/core'

const store = useSourcesStore()
const nodesStore = useNodesStore()
const recordings = useRecordingsStore()
const desk = useRecordDeskStore()
const router = useRouter()

const scanning = ref(false)
const ndiOpen = ref(false)
const filter = ref('')
const groupByNode = useStorage('cr.sources.groupByNode', true)

type Status = {
  key: 'live' | 'failed' | 'signal' | 'ok' | 'off'
  label: string
  dot: 'tally' | 'error' | 'warn' | 'ok' | 'off'
  /** A live source that isn't delivering frames, while recording. */
  note?: { label: string; dot: 'warn' | 'off' }
}

function status(s: Source): Status {
  const link = s.connected && s.link ? LINK[s.link] : null
  if (recordings.activeForSource(s.node_id, s.id)) {
    const note = link && link.dot !== 'ok' ? { label: link.label, dot: link.dot } : undefined
    return { key: 'live', label: 'Recording', dot: 'tally', note }
  }
  if (s.error) return { key: 'failed', label: 'Failed', dot: 'error' }
  if (link) return { key: link.dot === 'ok' ? 'ok' : link.dot === 'warn' ? 'signal' : 'off', label: link.label, dot: link.dot }
  if (s.connected) return { key: 'ok', label: 'Connected', dot: 'ok' }
  return { key: 'off', label: 'Not connected', dot: 'off' }
}

const STATUS_ORDER = { live: 0, failed: 1, signal: 2, ok: 3, off: 4 }

const columns: Column<Source>[] = [
  { id: 'status', label: 'Status', value: (s) => STATUS_ORDER[status(s).key], class: 'w-px' },
  { id: 'name', label: 'Name', value: (s) => s.display_name, alwaysVisible: true, class: 'min-w-40' },
  { id: 'type', label: 'Type', value: (s) => s.source_type, class: 'w-px' },
  { id: 'node', label: 'Node', value: (s) => nodesStore.nameOf(s.node_id), hiddenByDefault: true },
  { id: 'resolution', label: 'Resolution', value: (s) => (s.capabilities ? s.capabilities.max_width * s.capabilities.max_height : null), class: 'num whitespace-nowrap' },
  { id: 'fps', label: 'FPS', value: (s) => (s.capabilities ? s.capabilities.max_framerate[0] / s.capabilities.max_framerate[1] : null), class: 'num whitespace-nowrap' },
  { id: 'audio', label: 'Audio', value: (s) => s.capabilities?.audio_channels ?? null, class: 'num whitespace-nowrap' },
  { id: 'timecode', label: 'Timecode', value: (s) => s.timecode, hiddenByDefault: true, class: 'num whitespace-nowrap' },
  { id: 'id', label: 'ID', value: (s) => s.id, class: 'num' },
  { id: 'actions', label: '', alwaysVisible: true, align: 'right', class: 'w-px whitespace-nowrap' },
]

const grouped = computed(() => groupByNode.value && nodesStore.nodes.length > 1)

function openInRecord(s: Source) {
  desk.select(s.key)
  router.push('/record')
}

const formOpen = ref(false)
/** The source being edited; null when adding one. */
const editing = ref<Source | null>(null)

function openCreate() {
  editing.value = null
  formOpen.value = true
}

function openEdit(src: Source) {
  // Channels are set up in Playback.
  if (src.source_type === 'channel') {
    router.push({ path: '/playback', query: { channel: src.key, edit: '1' } })
    return
  }
  editing.value = src
  formOpen.value = true
}

// From the command palette: `?edit=<source key>` opens that source's settings.
const route = useRoute()
watch(
  () => route.query.edit,
  async (key) => {
    if (typeof key !== 'string') return
    await store.ensureLoaded()
    const src = store.sources.find((s) => s.key === key)
    router.replace({ query: {} })
    if (src) openEdit(src)
  },
  { immediate: true },
)

registerCommands(() => [
  { id: 'sources:add', title: 'Add source', group: 'This page', icon: Plus, keywords: ['new', 'create'], run: openCreate },
  { id: 'sources:ndi', title: 'Show NDI senders', group: 'This page', icon: RadioTower, keywords: ['ndi', 'network'], run: () => (ndiOpen.value = true) },
])

const deleting = ref<Source | null>(null)

async function destroy(src: Source) {
  try {
    await store.deleteSource(src.node_id, src.id)
    toast.success(`Deleted ${src.display_name}`)
  } catch (e) {
    notifyError(`Delete failed: ${src.display_name}`, e, src.node_id)
  }
}

/** Configured sources are deleted here; channels from their settings in Playback. */
const deletable = (s: Source) => CONFIGURED_TYPES.includes(s.source_type) && s.source_type !== 'channel'

const checked = useSelection()
const checkedSources = computed(() => store.sources.filter((s) => checked.selected.value.has(s.key)))
const bulkDeleting = ref(false)

const table = ref<{ displayedRows: Source[] }>()
registerCommands(() => [
  {
    id: 'sources:select-all',
    title: 'Select all sources shown',
    group: 'This page',
    icon: ListChecks,
    keywords: ['bulk', 'multiple', 'check'],
    run: () => checked.selectAll((table.value?.displayedRows ?? []).map((s) => s.key)),
  },
  ...(checked.count.value
    ? [{ id: 'sources:clear-selection', title: 'Clear selection', group: 'This page' as const, icon: X, keywords: ['deselect', 'none'], run: checked.clearSelection }]
    : []),
])

async function bulkDelete() {
  const result = await runBulk(checkedSources.value, (s) =>
    deletable(s)
      ? store.deleteSource(s.node_id, s.id)
      : Promise.reject(new BulkSkip(s.source_type === 'channel' ? 'channels are deleted in Playback' : 'NDI sources come from discovery')),
  )
  checked.deselect(result.ok.map((s) => s.key))
  bulkToast('Deleted', result, (s) => s.display_name)
}

async function scan() {
  scanning.value = true
  try {
    await store.scanWithToast()
  } finally {
    scanning.value = false
  }
}

onMounted(() => store.ensureLoaded())
</script>

<template>
  <PageHeader title="Sources" :count="store.sources.length">
    <template #actions>
      <Button size="sm" class="h-7 gap-1.5 text-xs" @click="openCreate"><Plus class="size-3.5" /> Add source</Button>
      <Button variant="outline" size="sm" class="h-7 gap-1.5 text-xs" :disabled="scanning" @click="scan">
        <RefreshCw class="size-3.5" :class="scanning && 'animate-spin'" /> Scan
      </Button>
      <Button variant="outline" size="sm" class="h-7 gap-1.5 text-xs" @click="ndiOpen = true">
        <RadioTower class="size-3.5" /> NDI senders
      </Button>
    </template>

    <ToolbarSearch v-model="filter" />

    <template #view>
      <ToolbarField v-if="nodesStore.nodes.length > 1" label="Group by node">
        <Switch v-model="groupByNode" />
      </ToolbarField>
      <ColumnsMenu table-id="sources" :columns="columns" />
    </template>
  </PageHeader>

  <div class="p-4 space-y-3">
    <LoadError :error="store.error" :retry="reloadAll" />
    <DataTable
      ref="table"
      table-id="sources"
      :columns="columns"
      :rows="store.sources"
      :row-key="(s) => s.key"
      :filter="filter"
      :group-by="grouped ? (s) => s.node_id : null"
      :group-label="(id) => nodesStore.labelOf(id)"
      :selection="checked"
    >
      <template #cell-status="{ row }">
        <span class="flex items-center gap-1.5 whitespace-nowrap" :class="status(row).key === 'live' && 'text-tally font-medium'">
          <StatusDot :status="status(row).dot" /> {{ status(row).label }}
        </span>
        <span
          v-if="status(row).note"
          class="flex items-center gap-1.5 whitespace-nowrap text-xs"
          :class="status(row).note!.dot === 'warn' ? 'text-warning' : 'text-muted-foreground'"
        >
          <StatusDot :status="status(row).note!.dot" /> {{ status(row).note!.label }}
        </span>
      </template>
      <template #cell-name="{ row }">
        <div class="font-medium text-sm">{{ row.display_name }}</div>
        <div v-if="store.alsoOn(row).length" class="text-xs text-muted-foreground">
          Also on {{ store.alsoOn(row).map(nodesStore.nameOf).join(', ') }}
        </div>
        <div v-if="row.error" class="text-destructive break-words">{{ row.error }}</div>
      </template>
      <template #cell-type="{ row }">
        <!-- Channels play out rather than capture: set apart, they're managed in Playback. -->
        <Badge v-if="row.source_type === 'channel'" variant="outline" class="uppercase" title="Plays out; managed in Playback">
          <MonitorPlay /> Playout channel
        </Badge>
        <Badge v-else variant="secondary" class="uppercase">{{ row.source_type }}</Badge>
      </template>
      <template #cell-node="{ row }">{{ nodesStore.labelOf(row.node_id) }}</template>
      <template #cell-resolution="{ row }">
        <template v-if="row.capabilities">{{ resolutionLabel(row.capabilities) }}</template>
        <span v-else class="text-muted-foreground font-sans" title="NDI and live sources get their format when they connect">on connect</span>
      </template>
      <template #cell-fps="{ row }">{{ row.capabilities ? fpsLabel(row.capabilities.max_framerate) : '—' }}</template>
      <template #cell-audio="{ row }">{{ row.capabilities ? `${row.capabilities.audio_channels} ch` : '—' }}</template>
      <template #cell-timecode="{ row }">{{ row.timecode ?? '—' }}</template>
      <template #cell-id="{ row }">
        <span class="flex items-center gap-1 text-muted-foreground">
          <span class="truncate max-w-28" :title="row.id">{{ row.id }}</span>
          <CopyButton :value="row.id" />
        </span>
      </template>
      <template #cell-actions="{ row }">
        <div class="flex justify-end gap-0.5">
          <Tooltip v-if="row.source_type === 'channel'">
            <TooltipTrigger as-child>
              <button class="row-btn" @click="router.push({ path: '/playback', query: { channel: row.key } })"><MonitorPlay class="size-3.5" /></button>
            </TooltipTrigger>
            <TooltipContent>Open in Playback</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger as-child>
              <button class="row-btn" @click="openInRecord(row)"><Radio class="size-3.5" /></button>
            </TooltipTrigger>
            <TooltipContent>Open in Record</TooltipContent>
          </Tooltip>
          <template v-if="CONFIGURED_TYPES.includes(row.source_type)">
            <Tooltip>
              <TooltipTrigger as-child>
                <button class="row-btn" @click="openEdit(row)"><Pencil class="size-3.5" /></button>
              </TooltipTrigger>
              <TooltipContent>{{ row.source_type === 'channel' ? 'Channel settings (in Playback)' : 'Edit source' }}</TooltipContent>
            </Tooltip>
            <!-- Channels are deleted from their settings in Playback. -->
            <Tooltip v-if="row.source_type !== 'channel'">
              <TooltipTrigger as-child>
                <button class="row-btn hover:text-destructive! hover:bg-destructive/10!" @click="deleting = row"><Trash2 class="size-3.5" /></button>
              </TooltipTrigger>
              <TooltipContent>Delete source</TooltipContent>
            </Tooltip>
          </template>
        </div>
      </template>
      <template #empty>
        <template v-if="!store.loaded">Loading sources…</template>
        <template v-else-if="store.error && !store.sources.length">No sources to show.</template>
        <template v-else-if="filter">No sources match the filter.</template>
        <template v-else>
          No sources yet. <button class="text-primary hover:underline" @click="scan">Scan for NDI sources</button>
          or <button class="text-primary hover:underline" @click="openCreate">add a source</button>.
        </template>
      </template>
    </DataTable>

    <BulkActionBar :count="checked.count.value" @clear="checked.clearSelection()">
      <Button variant="destructive" size="sm" class="h-7 gap-1.5 text-xs" @click="bulkDeleting = true">
        <Trash2 class="size-3.5" /> Delete
      </Button>
    </BulkActionBar>
  </div>

  <ConfirmDialog
    :open="!!deleting"
    :title="`Delete ${deleting?.display_name}?`"
    description="The source is removed from its node. Recordings already made are kept."
    @update:open="(v) => !v && (deleting = null)"
    @confirm="deleting && destroy(deleting)"
  />

  <ConfirmDialog
    v-model:open="bulkDeleting"
    :title="`Delete ${plural(checked.count.value, 'source')}?`"
    description="The sources are removed from their nodes. Recordings already made are kept. Playout channels and discovered NDI sources are skipped."
    @confirm="bulkDelete"
  />

  <SourceFormSheet v-if="formOpen" :source="editing" @close="formOpen = false" />
  <NdiSendersSheet v-if="ndiOpen" @close="ndiOpen = false" />
</template>

<style scoped>
@reference "@/style.css";
.row-btn {
  @apply size-7 grid place-items-center rounded text-muted-foreground hover:text-foreground hover:bg-accent;
}
</style>
