<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { useRouter } from 'vue-router'
import { toast } from 'vue-sonner'
import { Pencil, Radio, Trash2 } from '@lucide/vue'
import { useSourcesStore, type Source } from '@/stores/sources'
import { useNodesStore } from '@/stores/nodes'
import { useRecordingsStore } from '@/stores/recordings'
import { useRecordDeskStore } from '@/stores/recordDesk'
import { notifyError } from '@/lib/notify'
import { errorMessage } from '@/composables/useApi'
import { fpsLabel, resolutionLabel } from '@/lib/sourceFormat'
import type { TestSourceRequest } from '@/types/generated/TestSourceRequest'
import type { AudioTestSignal } from '@/types/generated/AudioTestSignal'
import type { VideoTestPattern } from '@/types/generated/VideoTestPattern'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import DataTable from '@/components/common/DataTable.vue'
import ColumnsMenu from '@/components/common/ColumnsMenu.vue'
import type { Column } from '@/components/common/dataTable'
import EditSheet from '@/components/common/EditSheet.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import CopyButton from '@/components/common/CopyButton.vue'
import StatusDot from '@/components/common/StatusDot.vue'
import { useStorage } from '@vueuse/core'

const store = useSourcesStore()
const nodesStore = useNodesStore()
const recordings = useRecordingsStore()
const desk = useRecordDeskStore()
const router = useRouter()
const nodes = computed(() => nodesStore.reachable)

const loading = ref(false)
const scanning = ref(false)
const filter = ref('')
const groupByNode = useStorage('cr.sources.groupByNode', true)

// ── Table ─────────────────────────────────────────────────────────────────────

type Status = { key: 'live' | 'failed' | 'ok' | 'off'; label: string; dot: 'tally' | 'error' | 'ok' | 'off' }

function status(s: Source): Status {
  if (recordings.activeForSource(s.node_id, s.id)) return { key: 'live', label: 'Recording', dot: 'tally' }
  if (s.error) return { key: 'failed', label: 'Failed', dot: 'error' }
  if (s.connected) return { key: 'ok', label: 'Connected', dot: 'ok' }
  return { key: 'off', label: 'Not connected', dot: 'off' }
}

const STATUS_ORDER = { live: 0, failed: 1, ok: 2, off: 3 }

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

// ── Test source form ──────────────────────────────────────────────────────────

const showForm = ref(false)
const editingId = ref<string | null>(null)
const saving = ref(false)
const formError = ref<string | null>(null)
const formNodeId = ref<string>('')  // which node to create on

const VIDEO_PATTERNS: { value: VideoTestPattern; label: string }[] = [
  { value: 'smpte',       label: 'SMPTE color bars' },
  { value: 'ball',        label: 'Moving ball' },
  { value: 'snow',        label: 'Snow' },
  { value: 'black',       label: 'Black' },
  { value: 'white',       label: 'White' },
  { value: 'smpte75',     label: 'SMPTE 75%' },
  { value: 'checkers-1',  label: 'Checkers' },
]

const AUDIO_SIGNALS: { value: AudioTestSignal; label: string }[] = [
  { value: 'tone',       label: 'Tone (sine)' },
  { value: 'silence',    label: 'Silence' },
  { value: 'pink-noise', label: 'Pink noise' },
]

const RESOLUTIONS = [
  { w: 1920, h: 1080, label: '1080p' },
  { w: 1280, h: 720,  label: '720p' },
  { w: 3840, h: 2160, label: '4K UHD' },
  { w: 720,  h: 576,  label: 'SD PAL' },
  { w: 720,  h: 486,  label: 'SD NTSC' },
]
const RESOLUTION_OPTIONS = RESOLUTIONS.map((r) => ({
  value: `${r.w}x${r.h}`,
  label: `${r.label} (${r.w}×${r.h})`,
}))

const FRAMERATES = [
  { n: 25,    d: 1,    label: '25 fps' },
  { n: 30,    d: 1,    label: '30 fps' },
  { n: 50,    d: 1,    label: '50 fps' },
  { n: 60,    d: 1,    label: '60 fps' },
  { n: 24000, d: 1001, label: '23.976 fps' },
  { n: 30000, d: 1001, label: '29.97 fps' },
]
const FRAMERATE_OPTIONS = FRAMERATES.map((r) => ({ value: `${r.n}/${r.d}`, label: r.label }))

const CHANNEL_OPTIONS = [
  { value: 1, label: 'Mono' },
  { value: 2, label: 'Stereo' },
  { value: 6, label: '5.1' },
  { value: 8, label: '7.1' },
]

const nodeOptions = computed(() => nodes.value.map((n) => ({ value: n.id, label: nodesStore.labelOf(n.id) })))

function blankForm(): TestSourceRequest {
  return {
    name: '',
    pattern: 'smpte',
    width: 1920,
    height: 1080,
    fps_num: 30,
    fps_den: 1,
    audio_signal: 'tone',
    frequency: 440,
    channels: 2,
  }
}

const form = reactive<TestSourceRequest>(blankForm())

const resolutionKey = computed({
  get: () => `${form.width}x${form.height}`,
  set: (v: string) => {
    const found = RESOLUTIONS.find((r) => `${r.w}x${r.h}` === v)
    if (found) { form.width = found.w; form.height = found.h }
  },
})

const framerateKey = computed({
  get: () => `${form.fps_num}/${form.fps_den}`,
  set: (v: string) => {
    const found = FRAMERATES.find((r) => `${r.n}/${r.d}` === v)
    if (found) { form.fps_num = found.n; form.fps_den = found.d }
  },
})

function openCreate() {
  editingId.value = null
  formNodeId.value = nodes.value.find((n) => n.is_self)?.id ?? nodes.value[0]?.id ?? ''
  Object.assign(form, blankForm())
  formError.value = null
  showForm.value = true
}

async function openEdit(src: Source) {
  formError.value = null
  let cfg
  try {
    cfg = (await store.testConfigs(src.node_id)).find((c) => c.id === src.id)
  } catch (e) {
    notifyError('Could not load test source config from node', e, src.node_id)
    return
  }
  if (!cfg) return
  editingId.value = src.id
  formNodeId.value = src.node_id
  const { id: _id, created_at: _created, ...config } = cfg
  Object.assign(form, config)
  showForm.value = true
}

async function save() {
  if (saving.value) return
  if (!form.name.trim()) {
    formError.value = 'Name is required.'
    return
  }
  saving.value = true
  formError.value = null
  try {
    if (editingId.value) {
      await store.updateTestSource(formNodeId.value, editingId.value, { ...form })
      toast.success(`Saved ${form.name}`)
    } else {
      await store.createTestSource(formNodeId.value, { ...form })
      toast.success(`Added ${form.name}`)
    }
    showForm.value = false
  } catch (e) {
    formError.value = errorMessage(e, 'Save failed.')
  } finally {
    saving.value = false
  }
}

// ── Delete ────────────────────────────────────────────────────────────────────

const deleting = ref<Source | null>(null)

async function destroy(src: Source) {
  try {
    await store.deleteTestSource(src.node_id, src.id)
    toast.success(`Deleted ${src.display_name}`)
  } catch (e) {
    notifyError(`Delete failed: ${src.display_name}`, e, src.node_id)
  }
}

// ── Scan ──────────────────────────────────────────────────────────────────────

async function scan() {
  scanning.value = true
  const before = new Set(store.sources.map((s) => s.key))
  try {
    await store.scanAll()
    const added = store.sources.filter((s) => !before.has(s.key)).length
    toast.success(`Scan complete — ${store.sources.length} sources`, {
      description: added ? `${added} new` : 'No new sources found',
    })
  } catch (e) {
    notifyError('Scan failed', e)
  } finally {
    scanning.value = false
  }
}

onMounted(async () => {
  if (store.sources.length) return
  loading.value = true
  try {
    await nodesStore.load()
    await store.loadSources()
  } finally {
    loading.value = false
  }
})
</script>

<template>
  <PageHeader title="Sources" :count="store.sources.length">
    <Input v-model="filter" placeholder="Filter…" class="h-7 w-44 text-xs" />
    <label v-if="nodesStore.nodes.length > 1" class="flex items-center gap-1.5 text-xs text-muted-foreground">
      <Switch v-model="groupByNode" /> Group by node
    </label>
    <ColumnsMenu table-id="sources" :columns="columns" />
    <div class="w-px h-5 bg-border mx-1" />
    <Button variant="outline" size="sm" class="h-7 text-xs" :disabled="scanning" @click="scan">
      {{ scanning ? 'Scanning…' : 'Scan' }}
    </Button>
    <Button size="sm" class="h-7 text-xs" @click="openCreate">Add test source</Button>
  </PageHeader>

  <div class="p-4">
    <DataTable
      table-id="sources"
      :columns="columns"
      :rows="store.sources"
      :row-key="(s) => s.key"
      :filter="filter"
      :group-by="grouped ? (s) => s.node_id : null"
      :group-label="(id) => nodesStore.labelOf(id)"
    >
      <template #cell-status="{ row }">
        <span class="flex items-center gap-1.5 whitespace-nowrap" :class="status(row).key === 'live' && 'text-tally font-medium'">
          <StatusDot :status="status(row).dot" /> {{ status(row).label }}
        </span>
      </template>
      <template #cell-name="{ row }">
        <div class="font-medium text-sm">{{ row.display_name }}</div>
        <div v-if="row.error" class="text-destructive break-words">{{ row.error }}</div>
      </template>
      <template #cell-type="{ row }">
        <Badge variant="secondary" class="uppercase">{{ row.source_type }}</Badge>
      </template>
      <template #cell-node="{ row }">{{ nodesStore.labelOf(row.node_id) }}</template>
      <template #cell-resolution="{ row }">
        <template v-if="row.capabilities">{{ resolutionLabel(row.capabilities) }}</template>
        <span v-else class="text-muted-foreground font-sans" title="NDI sources negotiate their format when connected">on connect</span>
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
          <Tooltip>
            <TooltipTrigger as-child>
              <button class="row-btn" @click="openInRecord(row)"><Radio class="size-3.5" /></button>
            </TooltipTrigger>
            <TooltipContent>Open in Record</TooltipContent>
          </Tooltip>
          <template v-if="row.source_type === 'test'">
            <Tooltip>
              <TooltipTrigger as-child>
                <button class="row-btn" @click="openEdit(row)"><Pencil class="size-3.5" /></button>
              </TooltipTrigger>
              <TooltipContent>Edit test source</TooltipContent>
            </Tooltip>
            <Tooltip>
              <TooltipTrigger as-child>
                <button class="row-btn hover:text-destructive! hover:bg-destructive/10!" @click="deleting = row"><Trash2 class="size-3.5" /></button>
              </TooltipTrigger>
              <TooltipContent>Delete test source</TooltipContent>
            </Tooltip>
          </template>
        </div>
      </template>
      <template #empty>
        <template v-if="loading">Loading sources…</template>
        <template v-else-if="filter">No sources match the filter.</template>
        <template v-else>
          No sources yet. <button class="text-primary hover:underline" @click="scan">Scan for NDI sources</button>
          or <button class="text-primary hover:underline" @click="openCreate">add a test source</button>.
        </template>
      </template>
    </DataTable>
  </div>

  <ConfirmDialog
    :open="!!deleting"
    :title="`Delete ${deleting?.display_name}?`"
    description="The test source is removed from its node. Recordings already made are kept."
    @update:open="(v) => !v && (deleting = null)"
    @confirm="deleting && destroy(deleting)"
  />

  <!-- Test source form -->
  <EditSheet
    v-if="showForm"
    :title="editingId ? 'Edit test source' : 'New test source'"
    description="A synthetic feed: a video pattern plus a test audio signal."
    :error="formError"
    :saving="saving"
    @close="showForm = false"
    @save="save"
  >
    <div class="grid grid-cols-2 gap-3">
      <FormField v-if="!editingId && nodes.length > 1" label="Node" class="col-span-2">
        <OptionSelect v-model="formNodeId" :options="nodeOptions" />
      </FormField>

      <FormField label="Name" class="col-span-2">
        <Input v-model="form.name" placeholder="Camera 1 sim" />
      </FormField>

      <FormField label="Video pattern" class="col-span-2">
        <OptionSelect v-model="form.pattern" :options="VIDEO_PATTERNS" />
      </FormField>

      <FormField label="Resolution">
        <OptionSelect v-model="resolutionKey" :options="RESOLUTION_OPTIONS" />
      </FormField>

      <FormField label="Frame rate">
        <OptionSelect v-model="framerateKey" :options="FRAMERATE_OPTIONS" />
      </FormField>

      <FormField label="Audio signal">
        <OptionSelect v-model="form.audio_signal" :options="AUDIO_SIGNALS" />
      </FormField>

      <FormField>
        <template #label>
          Frequency (Hz)
          <span v-if="form.audio_signal !== 'tone'" class="opacity-40">— n/a</span>
        </template>
        <Input
          v-model.number="form.frequency"
          type="number"
          :disabled="form.audio_signal !== 'tone'"
          placeholder="440"
          min="20"
          max="20000"
        />
      </FormField>

      <FormField label="Audio channels">
        <OptionSelect v-model="form.channels" :options="CHANNEL_OPTIONS" />
      </FormField>
    </div>
  </EditSheet>
</template>

<style scoped>
@reference "@/style.css";
.row-btn {
  @apply size-7 grid place-items-center rounded text-muted-foreground hover:text-foreground hover:bg-accent;
}
</style>
