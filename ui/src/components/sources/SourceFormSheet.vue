<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { toast } from 'vue-sonner'
import { ChevronLeft } from '@lucide/vue'
import { useSourcesStore, type ConfiguredKind, type Source } from '@/stores/sources'
import { useNodesStore } from '@/stores/nodes'
import { notifyError } from '@/lib/notify'
import { errorMessage } from '@/composables/useApi'
import { withPairedAudio } from '@/lib/deviceAudio'
import { useNodeDevices } from '@/composables/useNodeDevices'
import type { ConfiguredSourceRequest } from '@/types/generated/ConfiguredSourceRequest'
import type { DeviceSourceConfig } from '@/types/generated/DeviceSourceConfig'
import type { MediaInfo } from '@/types/generated/MediaInfo'
import type { StreamSourceConfig } from '@/types/generated/StreamSourceConfig'
import type { TestSourceConfig } from '@/types/generated/TestSourceConfig'
import type { WhipSourceConfig } from '@/types/generated/WhipSourceConfig'
import { Input } from '@/components/ui/input'
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'
import EditSheet from '@/components/common/EditSheet.vue'
import SourceTypeCards from './SourceTypeCards.vue'
import TestFields from './TestFields.vue'
import FileFields from './FileFields.vue'
import StreamFields from './StreamFields.vue'
import DeviceFields from './DeviceFields.vue'
import WhipFields from './WhipFields.vue'

/**
 * Add a configured source, or edit one (`source`). Mount it with `v-if`; an
 * edit opens once the source's config has come from its node. Channels are
 * set up in Playback, not here.
 */
const props = defineProps<{ source: Source | null }>()
const emit = defineEmits<{ close: [] }>()

const store = useSourcesStore()
const nodesStore = useNodesStore()
const nodes = computed(() => nodesStore.reachable)

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

/** False until an edit's config is in. */
const ready = ref(false)
const saving = ref(false)
const formError = ref<string | null>(null)
/** Which node to create on (or the edited source's). */
const nodeId = ref(props.source?.node_id ?? nodes.value.find((n) => n.is_self)?.id ?? nodes.value[0]?.id ?? '')
const nodeOptions = computed(() => nodes.value.map((n) => ({ value: n.id, label: nodesStore.labelOf(n.id) })))

/** `null` while picking the type of a new source. */
const kind = ref<Kind | null>(null)
const name = ref('')
const test = ref<TestSourceConfig>({
  pattern: 'smpte',
  width: 1920,
  height: 1080,
  fps_num: 30,
  fps_den: 1,
  audio_signal: 'tone',
  frequency: 440,
  channels: 2,
})
const filePath = ref('')
const fileMedia = ref<MediaInfo | null>(null)
const stream = ref<StreamSourceConfig>({ url: '', latency_ms: 200, rtsp_transport: 'auto', audio: { from: 'source', channels: 2 }, format: null })
const device = ref<DeviceSourceConfig>({ video_device: '', video_device_name: '', audio: { from: 'silence', channels: 2 }, format: null })
const whip = ref<WhipSourceConfig>({ port: 8890, audio: { from: 'source', channels: 2 }, format: null })

const node = computed(() => nodesStore.nodes.find((n) => n.id === nodeId.value))
/** The node's address as senders on the network see it. */
const host = computed(() => {
  const url = node.value?.url
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
  () => node.value?.source_types.find((t) => t.source_type === 'stream')?.protocols ?? ['rtsp', 'srt', 'rtmp', 'http', 'https', 'udp'],
)
/** Kinds the node can't run, and why. */
const unavailable = computed(() => {
  const out: Partial<Record<Kind, string>> = {}
  for (const t of node.value?.source_types ?? []) {
    if (t.missing.length && t.source_type !== 'ndi') {
      out[t.source_type] = `Not available: this node lacks ${t.missing.join(', ')}`
    }
  }
  return out
})
const nodeDevices = useNodeDevices(nodeId)

function pickKind(k: Kind) {
  kind.value = k
  formError.value = null
  const picked = nodeId.value
  if (k === 'device' || k === 'stream' || k === 'whip') nodeDevices.refresh().then(() => {
    // Preselect the first camera for a new device source (on the node still picked).
    if (k === 'device' && !device.value.video_device && nodeId.value === picked) {
      const first = nodeDevices.devices.value.find((d) => d.kind === 'video' || d.kind === 'screen')
      if (first) {
        device.value = {
          ...device.value,
          video_device: first.key,
          audio: withPairedAudio(device.value.audio, first, nodeDevices.devices.value),
        }
      }
    }
  })
}

onMounted(async () => {
  const src = props.source
  if (!src) {
    ready.value = true
    return
  }
  let cfg
  try {
    cfg = await store.getConfig(src.node_id, src.id)
  } catch (e) {
    notifyError('Could not load the source config from its node', e, src.node_id)
  }
  if (!cfg) return emit('close')
  name.value = cfg.name
  kind.value = cfg.config.type
  const c = cfg.config
  if (c.type === 'test') {
    const { type: _type, ...rest } = c
    test.value = rest
  } else if (c.type === 'file') {
    filePath.value = c.path
    fileMedia.value = c.media
  } else if (c.type === 'stream') {
    const { type: _type, ...rest } = c
    stream.value = rest
  } else if (c.type === 'device') {
    const { type: _type, ...rest } = c
    device.value = rest
  } else if (c.type === 'whip') {
    const { type: _type, ...rest } = c
    whip.value = rest
  }
  ready.value = true
  if (c.type === 'stream' || c.type === 'device' || c.type === 'whip') nodeDevices.refresh()
})

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
    default:
      return { type: 'test', ...test.value }
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
    if (props.source) {
      await store.updateSource(nodeId.value, props.source.id, req)
      toast.success(`Saved ${req.name}`)
    } else {
      await store.createSource(nodeId.value, req)
      toast.success(`Added ${req.name}`)
    }
    emit('close')
  } catch (e) {
    formError.value = errorMessage(e, 'Save failed.')
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <EditSheet
    v-if="ready"
    :title="`${source ? 'Edit' : 'New'} ${kind ? KIND_TITLES[kind] : 'source'}`"
    :description="kind ? KIND_DESCRIPTIONS[kind] : 'What kind of source?'"
    :error="formError"
    :saving="saving"
    :no-save="!kind"
    @close="emit('close')"
    @save="save"
  >
    <div class="grid grid-cols-2 gap-3">
      <FormField v-if="!source && nodes.length > 1" label="Node" class="col-span-2">
        <OptionSelect v-model="nodeId" :options="nodeOptions" />
      </FormField>

      <template v-if="!kind">
        <SourceTypeCards class="col-span-2" :unavailable="unavailable" @pick="pickKind" />
      </template>
      <button
        v-else-if="!source"
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
        :host="host"
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
      <WhipFields v-else-if="kind === 'whip'" v-model="whip" :host="host" :devices="nodeDevices.devices.value" />
      <FileFields v-else-if="kind === 'file'" v-model:path="filePath" v-model:media="fileMedia" :node-id="nodeId" />
      <TestFields v-else-if="kind === 'test'" v-model="test" />
    </div>
  </EditSheet>
</template>
