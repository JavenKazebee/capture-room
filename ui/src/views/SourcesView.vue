<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { useSourcesStore, type Source } from '@/stores/sources'
import { errorMessage } from '@/composables/useApi'
import type { TestSourceRequest } from '@/types/generated/TestSourceRequest'
import { useNodesStore } from '@/stores/nodes'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import FormField from '@/components/FormField.vue'
import FormModal from '@/components/FormModal.vue'
import OptionSelect from '@/components/OptionSelect.vue'
import type { AudioTestSignal } from '@/types/generated/AudioTestSignal'
import type { VideoTestPattern } from '@/types/generated/VideoTestPattern'

const store = useSourcesStore()
const nodesStore = useNodesStore()
const nodes = computed(() => nodesStore.reachable)

const loading = ref(false)
const scanning = ref(false)
const error = ref<string | null>(null)

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

const nodeOptions = computed(() =>
  nodes.value.map((n) => ({ value: n.id, label: n.is_self ? `${n.name} (this node)` : n.name })),
)

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
  } catch {
    error.value = 'Could not load test source config from node.'
    return
  }
  if (!cfg) return
  editingId.value = src.id
  formNodeId.value = src.node_id
  const { id: _id, created_at: _created, ...config } = cfg
  Object.assign(form, config)
  showForm.value = true
}

function closeForm() {
  showForm.value = false
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
    } else {
      await store.createTestSource(formNodeId.value, { ...form })
    }
    showForm.value = false
  } catch (e) {
    formError.value = errorMessage(e, 'Save failed.')
  } finally {
    saving.value = false
  }
}

async function destroy(src: Source) {
  try {
    await store.deleteTestSource(src.node_id, src.id)
  } catch (e) {
    error.value = errorMessage(e, 'Delete failed.')
  }
}

// ── Scan ──────────────────────────────────────────────────────────────────────

async function scan() {
  scanning.value = true
  error.value = null
  try {
    await store.scanAll()
  } catch (e) {
    error.value = errorMessage(e, 'Scan failed.')
  } finally {
    scanning.value = false
  }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

function fpsLabel(n: number, d: number) {
  return d === 1 ? `${n} fps` : `${(n / d).toFixed(3)} fps`
}

function nodeName(nodeId: string) {
  const n = nodesStore.nodes.find((nd) => nd.id === nodeId)
  return n ? (n.is_self ? `${n.name} (this node)` : n.name) : nodeId
}

const sourcesByNode = computed(() => {
  const map = new Map<string, Source[]>()
  for (const s of store.sources) {
    if (!map.has(s.node_id)) map.set(s.node_id, [])
    map.get(s.node_id)!.push(s)
  }
  return map
})

onMounted(async () => {
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
  <div class="p-6 max-w-5xl">
    <!-- Header -->
    <div class="flex items-center justify-between mb-6">
      <h1 class="text-2xl font-semibold">Sources</h1>
      <div class="flex gap-2">
        <Button variant="outline" size="default" :disabled="scanning" @click="scan">
          {{ scanning ? 'Scanning…' : 'Scan' }}
        </Button>
        <Button size="default" @click="openCreate">Add test source</Button>
      </div>
    </div>

    <p v-if="error" class="text-sm text-destructive mb-4">{{ error }}</p>

    <div v-if="loading" class="text-center text-muted-foreground py-16">Loading sources…</div>

    <div
      v-else-if="store.sources.length === 0"
      class="text-center text-muted-foreground py-16 rounded-lg border border-dashed border-border"
    >
      No sources found. Click <strong>Scan</strong> to discover sources.
    </div>

    <!-- Source list, grouped by node -->
    <div v-else class="space-y-6">
      <div v-for="[nodeId, nodeSources] in sourcesByNode" :key="nodeId">
        <h2
          v-if="sourcesByNode.size > 1"
          class="text-xs font-medium text-muted-foreground uppercase tracking-wider mb-2"
        >
          {{ nodeName(nodeId) }}
        </h2>

        <div class="rounded-lg border border-border bg-card divide-y divide-border">
          <div v-for="src in nodeSources" :key="src.key" class="p-4">
            <div class="flex items-start gap-4">
              <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2 mb-1">
                  <span class="font-medium text-sm truncate">{{ src.display_name }}</span>
                  <Badge variant="secondary" class="shrink-0">{{ src.source_type }}</Badge>
                </div>
                <div class="text-xs text-muted-foreground flex flex-wrap gap-x-3 gap-y-0.5">
                  <span>{{ src.capabilities.max_width }}×{{ src.capabilities.max_height }}</span>
                  <span>{{ fpsLabel(src.capabilities.max_framerate[0], src.capabilities.max_framerate[1]) }}</span>
                  <span>{{ src.capabilities.audio_channels }}ch audio</span>
                  <span class="font-mono opacity-60">{{ src.id }}</span>
                </div>
              </div>

              <!-- Edit / Delete for all test sources -->
              <div v-if="src.source_type === 'test'" class="flex gap-2 shrink-0">
                <Button variant="outline" size="default" @click="openEdit(src)">
                  Edit
                </Button>
                <Button
                  variant="destructive"
                  size="default"
                  @click="destroy(src)"
                >
                  Delete
                </Button>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- Test source form modal -->
    <FormModal
      v-if="showForm"
      :title="editingId ? 'Edit test source' : 'New test source'"
      :error="formError"
      :saving="saving"
      @close="closeForm"
      @save="save"
    >
      <div class="grid grid-cols-2 gap-3">
        <!-- Node selector — only for new sources when multiple nodes exist -->
        <FormField v-if="!editingId && nodes.length > 1" label="Node" class="col-span-2">
          <OptionSelect v-model="formNodeId" :options="nodeOptions" />
        </FormField>

        <FormField label="Name" class="col-span-2">
          <Input v-model="form.name" placeholder="e.g. Camera 1 Sim" />
        </FormField>

        <FormField label="Video pattern" class="col-span-2">
          <OptionSelect v-model="form.pattern" :options="VIDEO_PATTERNS" />
        </FormField>

        <FormField label="Resolution">
          <OptionSelect v-model="resolutionKey" :options="RESOLUTION_OPTIONS" />
        </FormField>

        <FormField label="Framerate">
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
    </FormModal>
  </div>
</template>
