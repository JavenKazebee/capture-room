<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { toast } from 'vue-sonner'
import { ChevronLeft, FolderOpen, MonitorPlay, Pencil, Plus, Radio, RefreshCw, Trash2 } from '@lucide/vue'
import { CONFIGURED_TYPES, useSourcesStore, type ConfiguredKind, type Source } from '@/stores/sources'
import { useNodesStore } from '@/stores/nodes'
import { useRecordingsStore } from '@/stores/recordings'
import { useRecordDeskStore } from '@/stores/recordDesk'
import { notifyError } from '@/lib/notify'
import { errorMessage } from '@/composables/useApi'
import { fpsLabel, resolutionLabel } from '@/lib/sourceFormat'
import { formatDuration } from '@/lib/format'
import type { TestSourceConfig } from '@/types/generated/TestSourceConfig'
import type { MediaInfo } from '@/types/generated/MediaInfo'
import type { ConfiguredSourceRequest } from '@/types/generated/ConfiguredSourceRequest'
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
import ToolbarField from '@/components/common/ToolbarField.vue'
import ToolbarSearch from '@/components/common/ToolbarSearch.vue'
import DataTable from '@/components/common/DataTable.vue'
import ColumnsMenu from '@/components/common/ColumnsMenu.vue'
import type { Column } from '@/components/common/dataTable'
import EditSheet from '@/components/common/EditSheet.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import CopyButton from '@/components/common/CopyButton.vue'
import StatusDot from '@/components/common/StatusDot.vue'
import FileBrowseDialog from '@/components/sources/FileBrowseDialog.vue'
import SourceTypeCards from '@/components/sources/SourceTypeCards.vue'
import StreamFields from '@/components/sources/StreamFields.vue'
import DeviceFields from '@/components/sources/DeviceFields.vue'
import WhipFields from '@/components/sources/WhipFields.vue'
import ChannelFields from '@/components/sources/ChannelFields.vue'
import { useNodeDevices } from '@/composables/useNodeDevices'
import { LINK } from '@/lib/linkState'
import { FRAMERATES, FRAMERATE_OPTIONS, RESOLUTIONS, RESOLUTION_OPTIONS } from '@/lib/videoPresets'
import type { StreamSourceConfig } from '@/types/generated/StreamSourceConfig'
import type { DeviceSourceConfig } from '@/types/generated/DeviceSourceConfig'
import type { WhipSourceConfig } from '@/types/generated/WhipSourceConfig'
import type { ChannelConfig } from '@/types/generated/ChannelConfig'
import { useStorage } from '@vueuse/core'

const store = useSourcesStore()
const nodesStore = useNodesStore()
const recordings = useRecordingsStore()
const desk = useRecordDeskStore()
const router = useRouter()
const route = useRoute()
const nodes = computed(() => nodesStore.reachable)

const loading = ref(false)
const scanning = ref(false)
const filter = ref('')
const groupByNode = useStorage('cr.sources.groupByNode', true)

// ── Table ─────────────────────────────────────────────────────────────────────

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

// ── Source form ───────────────────────────────────────────────────────────────

type Kind = ConfiguredKind

const NAME_PLACEHOLDERS: Record<Kind, string> = {
  test: 'Camera 1 sim',
  file: 'Camera 1 footage',
  stream: 'Stage cam (RTSP)',
  device: 'Cam Link 1',
  whip: 'Remote guest',
  channel: 'Playout A',
}

/** As they read after "New" / "Edit". */
const KIND_TITLES: Record<Kind, string> = {
  test: 'test pattern',
  file: 'media file',
  stream: 'network stream',
  device: 'capture device',
  whip: 'WHIP ingest',
  channel: 'playout channel',
}

const KIND_DESCRIPTIONS: Record<Kind, string> = {
  test: 'A synthetic feed: a video pattern plus a test audio signal.',
  file: "A media file on the node's disk, played in a loop as a live feed.",
  stream: 'A network stream. If it drops, the feed plays black and silence and recordings keep going until it reconnects.',
  device: 'A camera, capture card or screen on the node, with audio from one of its audio inputs.',
  whip: 'An endpoint on the node that OBS or a browser publishes WebRTC to. Black and silence while nobody publishes.',
  channel: 'Plays clips from the media library out at a fixed format, operated from Playback. Black and silence when nothing plays.',
}

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

const CHANNEL_OPTIONS = [
  { value: 1, label: 'Mono' },
  { value: 2, label: 'Stereo' },
  { value: 6, label: '5.1' },
  { value: 8, label: '7.1' },
]

const nodeOptions = computed(() => nodes.value.map((n) => ({ value: n.id, label: nodesStore.labelOf(n.id) })))

function blankTest(): TestSourceConfig {
  return {
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

/** `null` while picking the type of a new source. */
const kind = ref<Kind | null>('test')
const name = ref('')
const form = reactive<TestSourceConfig>(blankTest())
const filePath = ref('')
/** What the node found in the file when it was last saved. */
const fileMedia = ref<MediaInfo | null>(null)
const browsing = ref(false)

function blankStream(): StreamSourceConfig {
  return { url: '', latency_ms: 200, rtsp_transport: 'auto', audio: { from: 'source', channels: 2 }, format: null }
}
function blankDevice(): DeviceSourceConfig {
  return { video_device: '', video_device_name: '', audio: { from: 'silence', channels: 2 }, format: null }
}
function blankWhip(): WhipSourceConfig {
  return { port: 8890, audio: { from: 'source', channels: 2 }, format: null }
}
function blankChannel(): ChannelConfig {
  return { format: { width: 1920, height: 1080, fps_num: 30, fps_den: 1 }, audio_channels: 2, outputs: [{ type: 'ndi', ndi_name: null }] }
}
const stream = ref<StreamSourceConfig>(blankStream())
const device = ref<DeviceSourceConfig>(blankDevice())
const whip = ref<WhipSourceConfig>(blankWhip())
const channel = ref<ChannelConfig>(blankChannel())

const formNode = computed(() => nodesStore.nodes.find((n) => n.id === formNodeId.value))
/** The node's address as senders on the network see it. */
const formHost = computed(() => {
  const url = formNode.value?.url
  if (url) {
    try {
      return new URL(url).hostname
    } catch {
      // Fall through to the UI's own host.
    }
  }
  return window.location.hostname
})
const protocols = computed(
  () => formNode.value?.source_types.find((t) => t.source_type === 'stream')?.protocols ?? ['rtsp', 'srt', 'rtmp', 'http', 'https', 'udp'],
)
/** Output types the node's channels can send. */
const outputTypes = computed(
  () => formNode.value?.source_types.find((t) => t.source_type === 'channel')?.protocols ?? ['ndi', 'srt', 'rtsp'],
)
/** Kinds the node can't run, and why. */
const unavailable = computed(() => {
  const out: Partial<Record<Kind, string>> = {}
  for (const t of formNode.value?.source_types ?? []) {
    if (t.missing.length && t.source_type !== 'ndi') {
      out[t.source_type] = `Not available: this node lacks ${t.missing.join(', ')}`
    }
  }
  return out
})
const nodeDevices = useNodeDevices(formNodeId)

function mediaSummary(m: MediaInfo) {
  const parts = [`${m.width}×${m.height}`, fpsLabel([m.fps_num, m.fps_den])]
  parts.push(m.audio_channels ? `${m.audio_channels} ch audio` : 'no audio (plays silence)')
  if (m.duration_ms != null) parts.push(`${formatDuration(m.duration_ms)} loop`)
  return parts.join(' · ')
}

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
  kind.value = null
  name.value = ''
  Object.assign(form, blankTest())
  filePath.value = ''
  fileMedia.value = null
  stream.value = blankStream()
  device.value = blankDevice()
  whip.value = blankWhip()
  channel.value = blankChannel()
  formError.value = null
  showForm.value = true
}

function pickKind(k: Kind) {
  kind.value = k
  formError.value = null
  if (k === 'device' || k === 'stream' || k === 'whip') nodeDevices.refresh().then(() => {
    // Preselect the first camera for a new device source.
    if (k === 'device' && !device.value.video_device) {
      const first = nodeDevices.devices.value.find((d) => d.kind !== 'audio')
      if (first) device.value = { ...device.value, video_device: first.key }
    }
  })
}

async function openEdit(src: Source) {
  formError.value = null
  let cfg
  try {
    cfg = (await store.configs(src.node_id)).find((c) => c.id === src.id)
  } catch (e) {
    notifyError('Could not load the source config from its node', e, src.node_id)
    return
  }
  if (!cfg) return
  editingId.value = src.id
  formNodeId.value = src.node_id
  name.value = cfg.name
  kind.value = cfg.config.type
  const c = cfg.config
  if (c.type === 'test') {
    const { type: _type, ...test } = c
    Object.assign(form, test)
  } else if (c.type === 'file') {
    filePath.value = c.path
    fileMedia.value = c.media
  } else if (c.type === 'stream') {
    const { type: _type, ...rest } = c
    stream.value = rest
  } else if (c.type === 'device') {
    const { type: _type, ...rest } = c
    device.value = rest
  } else if (c.type === 'channel') {
    const { type: _type, ...rest } = c
    channel.value = rest
  } else {
    const { type: _type, ...rest } = c
    whip.value = rest
  }
  showForm.value = true
  if (c.type === 'stream' || c.type === 'device' || c.type === 'whip') nodeDevices.refresh()
}

/** What's missing before the node is asked, or null. The node checks the rest. */
function formProblem(): string | null {
  switch (kind.value) {
    case 'file':
      return filePath.value.trim() ? null : 'Choose a file.'
    case 'stream':
      return stream.value.url.trim() ? audioProblem(stream.value.audio) : 'Enter the stream URL.'
    case 'device':
      return device.value.video_device ? audioProblem(device.value.audio) : 'Pick a video device.'
    case 'whip':
      return whip.value.port >= 1024 && whip.value.port <= 65535
        ? audioProblem(whip.value.audio)
        : 'Use a port from 1024 to 65535.'
    default:
      return null
  }
}

function audioProblem(audio: StreamSourceConfig['audio']) {
  if (audio.from !== 'device') return null
  if (!audio.device_key) return 'Pick an audio input.'
  return audio.channels.length ? null : 'Pick at least one audio channel.'
}

function formConfig(): ConfiguredSourceRequest['config'] {
  switch (kind.value) {
    case 'file':
      return { type: 'file', path: filePath.value, media: null }
    case 'stream':
      return { type: 'stream', ...stream.value, url: stream.value.url.trim() }
    case 'device':
      return { type: 'device', ...device.value }
    case 'whip':
      return { type: 'whip', ...whip.value }
    case 'channel':
      return { type: 'channel', ...channel.value }
    default:
      return { type: 'test', ...form }
  }
}

async function save() {
  if (saving.value) return
  if (!name.value.trim()) {
    formError.value = 'Name is required.'
    return
  }
  const problem = formProblem()
  if (problem) {
    formError.value = problem
    return
  }
  const req: ConfiguredSourceRequest = { name: name.value, config: formConfig() }
  saving.value = true
  formError.value = null
  try {
    if (editingId.value) {
      await store.updateSource(formNodeId.value, editingId.value, req)
      toast.success(`Saved ${req.name}`)
    } else {
      await store.createSource(formNodeId.value, req)
      toast.success(`Added ${req.name}`)
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
    await store.deleteSource(src.node_id, src.id)
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
      id: 'scan',
      description: added ? `${added} new` : 'No new sources found',
    })
  } catch (e) {
    notifyError('Scan failed', e)
  } finally {
    scanning.value = false
  }
}

// `?add=channel` (from Playback): open the form on that type.
watch(
  () => route.query.add,
  async (add) => {
    if (add !== 'channel') return
    router.replace({ query: {} })
    if (!nodes.value.length) await nodesStore.load()
    openCreate()
    pickKind('channel')
  },
  { immediate: true },
)

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
    <template #actions>
      <Button size="sm" class="h-7 gap-1.5 text-xs" @click="openCreate"><Plus class="size-3.5" /> Add source</Button>
      <Button variant="outline" size="sm" class="h-7 gap-1.5 text-xs" :disabled="scanning" @click="scan">
        <RefreshCw class="size-3.5" :class="scanning && 'animate-spin'" /> Scan
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
        <div v-if="row.error" class="text-destructive break-words">{{ row.error }}</div>
      </template>
      <template #cell-type="{ row }">
        <Badge variant="secondary" class="uppercase">{{ row.source_type }}</Badge>
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
              <TooltipContent>Edit source</TooltipContent>
            </Tooltip>
            <Tooltip>
              <TooltipTrigger as-child>
                <button class="row-btn hover:text-destructive! hover:bg-destructive/10!" @click="deleting = row"><Trash2 class="size-3.5" /></button>
              </TooltipTrigger>
              <TooltipContent>Delete source</TooltipContent>
            </Tooltip>
          </template>
        </div>
      </template>
      <template #empty>
        <template v-if="loading">Loading sources…</template>
        <template v-else-if="filter">No sources match the filter.</template>
        <template v-else>
          No sources yet. <button class="text-primary hover:underline" @click="scan">Scan for NDI sources</button>
          or <button class="text-primary hover:underline" @click="openCreate">add a source</button>.
        </template>
      </template>
    </DataTable>
  </div>

  <ConfirmDialog
    :open="!!deleting"
    :title="`Delete ${deleting?.display_name}?`"
    description="The source is removed from its node. Recordings already made are kept."
    @update:open="(v) => !v && (deleting = null)"
    @confirm="deleting && destroy(deleting)"
  />

  <!-- Source form -->
  <EditSheet
    v-if="showForm"
    :title="`${editingId ? 'Edit' : 'New'} ${kind ? KIND_TITLES[kind] : 'source'}`"
    :description="kind ? KIND_DESCRIPTIONS[kind] : 'What kind of source?'"
    :error="formError"
    :saving="saving"
    :no-save="!kind"
    @close="showForm = false"
    @save="save"
  >
    <div class="grid grid-cols-2 gap-3">
      <FormField v-if="!editingId && nodes.length > 1" label="Node" class="col-span-2">
        <OptionSelect v-model="formNodeId" :options="nodeOptions" />
      </FormField>

      <template v-if="!kind">
        <SourceTypeCards class="col-span-2" :unavailable="unavailable" @pick="pickKind" />
      </template>
      <button
        v-else-if="!editingId"
        type="button"
        class="col-span-2 -mt-1 flex items-center gap-0.5 text-xs text-muted-foreground hover:text-foreground w-fit"
        @click="kind = null"
      >
        <ChevronLeft class="size-3.5" /> Other source types
      </button>

      <FormField v-if="kind" label="Name" class="col-span-2">
        <Input v-model="name" :placeholder="NAME_PLACEHOLDERS[kind]" />
      </FormField>

      <StreamFields
        v-if="kind === 'stream'"
        v-model="stream"
        :protocols="protocols"
        :host="formHost"
        :devices="nodeDevices.devices.value"
      />
      <DeviceFields
        v-else-if="kind === 'device'"
        v-model="device"
        :devices="nodeDevices.devices.value"
        :loading="nodeDevices.loading.value"
        :error="nodeDevices.error.value"
        @refresh="nodeDevices.refresh"
      />
      <WhipFields v-else-if="kind === 'whip'" v-model="whip" :host="formHost" :devices="nodeDevices.devices.value" />
      <ChannelFields
        v-else-if="kind === 'channel'"
        v-model="channel"
        :name="name"
        :host="formHost"
        :output-types="outputTypes"
      />

      <template v-if="kind === 'file'">
        <FormField label="File" class="col-span-2">
          <div class="flex gap-1.5">
            <Input
              v-model="filePath"
              class="font-mono text-[0.6875rem]"
              placeholder="/path/on/the/node/clip.mov"
              @update:model-value="fileMedia = null"
            />
            <Button type="button" variant="outline" class="gap-1.5" @click="browsing = true">
              <FolderOpen class="size-3.5" /> Browse
            </Button>
          </div>
        </FormField>
        <p class="col-span-2 text-xs text-muted-foreground">
          <template v-if="fileMedia">{{ mediaSummary(fileMedia) }}</template>
          <template v-else>The node checks the file when you save. Files without audio play silence.</template>
        </p>
      </template>

      <template v-else-if="kind === 'test'">
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
      </template>
    </div>
  </EditSheet>

  <FileBrowseDialog
    v-if="browsing"
    :node-id="formNodeId"
    :start="filePath"
    @close="browsing = false"
    @select="(p) => { filePath = p; fileMedia = null }"
  />
</template>

<style scoped>
@reference "@/style.css";
.row-btn {
  @apply size-7 grid place-items-center rounded text-muted-foreground hover:text-foreground hover:bg-accent;
}
</style>
