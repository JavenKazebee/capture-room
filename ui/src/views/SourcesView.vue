<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { useSourcesStore, type TestSourceConfig, type TestSourceInput } from '@/stores/sources'
import { useApi } from '@/composables/useApi'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'

const store = useSourcesStore()
const { api } = useApi()

const loading = ref(false)
const scanning = ref(false)
const error = ref<string | null>(null)
const localNodeId = ref<string | null>(null)

interface NodeOption { id: string; name: string; is_self: boolean }
const nodes = ref<NodeOption[]>([])

// ── Test source form ──────────────────────────────────────────────────────────

const showForm = ref(false)
const editingId = ref<string | null>(null)
const saving = ref(false)
const formError = ref<string | null>(null)
const formNodeId = ref<string>('')  // which node to create on

const VIDEO_PATTERNS = [
  { value: 'smpte',       label: 'SMPTE color bars' },
  { value: 'ball',        label: 'Moving ball' },
  { value: 'snow',        label: 'Snow' },
  { value: 'black',       label: 'Black' },
  { value: 'white',       label: 'White' },
  { value: 'smpte75',     label: 'SMPTE 75%' },
  { value: 'checkers-1',  label: 'Checkers' },
]

const AUDIO_SIGNALS = [
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

const FRAMERATES = [
  { n: 25,    d: 1,    label: '25 fps' },
  { n: 30,    d: 1,    label: '30 fps' },
  { n: 50,    d: 1,    label: '50 fps' },
  { n: 60,    d: 1,    label: '60 fps' },
  { n: 24000, d: 1001, label: '23.976 fps' },
  { n: 30000, d: 1001, label: '29.97 fps' },
]

function blankForm(): TestSourceInput {
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

const form = reactive<TestSourceInput>(blankForm())

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
  formNodeId.value = localNodeId.value ?? ''
  Object.assign(form, blankForm())
  formError.value = null
  showForm.value = true
}

async function openEdit(localSrcId: string, nodeId: string) {
  formError.value = null
  let cfg
  if (nodeId === localNodeId.value) {
    cfg = store.testConfigs.find((c) => c.id === localSrcId)
  } else {
    try {
      const remoteConfigs = await api<TestSourceConfig[]>(
        `/sources/test?node_id=${encodeURIComponent(nodeId)}`,
      )
      cfg = remoteConfigs.find((c) => c.id === localSrcId)
    } catch {
      error.value = 'Could not load config from remote node.'
      return
    }
  }
  if (!cfg) return
  editingId.value = localSrcId
  formNodeId.value = nodeId
  Object.assign(form, {
    name: cfg.name,
    pattern: cfg.pattern,
    width: cfg.width,
    height: cfg.height,
    fps_num: cfg.fps_num,
    fps_den: cfg.fps_den,
    audio_signal: cfg.audio_signal,
    frequency: cfg.frequency,
    channels: cfg.channels,
  })
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
    const targetNode = formNodeId.value || undefined
    if (editingId.value) {
      await store.updateTestSource(editingId.value, { ...form }, targetNode)
    } else {
      await store.createTestSource({ ...form }, targetNode)
    }
    showForm.value = false
  } catch (e) {
    formError.value = e instanceof Error ? e.message : 'Save failed.'
  } finally {
    saving.value = false
  }
}

async function destroy(localSrcId: string, name: string, nodeId: string) {
  try {
    await store.deleteTestSource(localSrcId, nodeId)
  } catch (e) {
    error.value = e instanceof Error ? e.message : 'Delete failed.'
  }
}

function localId(compositeId: string) {
  return compositeId.includes(':') ? compositeId.split(':')[1] : compositeId
}

// ── Scan ──────────────────────────────────────────────────────────────────────

async function scan() {
  scanning.value = true
  error.value = null
  try {
    await store.scan()
  } catch (e) {
    error.value = e instanceof Error ? e.message : 'Scan failed.'
  } finally {
    scanning.value = false
  }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

function fpsLabel(n: number, d: number) {
  return d === 1 ? `${n} fps` : `${(n / d).toFixed(3)} fps`
}

function nodeName(nodeId: string) {
  const n = nodes.value.find((nd) => nd.id === nodeId)
  return n ? (n.is_self ? `${n.name} (this node)` : n.name) : nodeId
}

const sourcesByNode = computed(() => {
  const map = new Map<string, typeof store.sources>()
  for (const s of store.sources) {
    const nodeId = s.node_id ?? 'local'
    if (!map.has(nodeId)) map.set(nodeId, [])
    map.get(nodeId)!.push(s)
  }
  return map
})

const fieldClass =
  'h-8 w-full rounded-md border border-border bg-background px-2 text-sm outline-none focus:ring-2 focus:ring-ring/30'

onMounted(async () => {
  loading.value = true
  try {
    const [, nodeList, status] = await Promise.all([
      Promise.all([store.loadSources(), store.loadTestConfigs()]),
      api<NodeOption[]>('/nodes').catch(() => [] as NodeOption[]),
      api<{ id: string }>('/status').catch(() => null),
    ])
    nodes.value = nodeList
    localNodeId.value = status?.id ?? null
    formNodeId.value = localNodeId.value ?? ''
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
          <div v-for="src in nodeSources" :key="src.id" class="p-4">
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
                <Button variant="outline" size="default" @click="openEdit(localId(src.id), src.node_id ?? localNodeId ?? '')">
                  Edit
                </Button>
                <Button
                  variant="destructive"
                  size="default"
                  @click="destroy(localId(src.id), src.display_name, src.node_id ?? localNodeId ?? '')"
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
    <div
      v-if="showForm"
      class="fixed inset-0 bg-black/40 flex items-center justify-center p-4 z-50"
      @click.self="closeForm"
    >
      <div class="bg-card border border-border rounded-lg w-full max-w-lg max-h-[90vh] overflow-y-auto p-5">
        <h2 class="text-lg font-semibold mb-4">
          {{ editingId ? 'Edit test source' : 'New test source' }}
        </h2>

        <div class="grid grid-cols-2 gap-3">
          <!-- Node selector — only for new sources when multiple nodes exist -->
          <label v-if="!editingId && nodes.length > 1" class="col-span-2 flex flex-col gap-1">
            <span class="text-xs text-muted-foreground">Node</span>
            <select v-model="formNodeId" :class="fieldClass">
              <option v-for="n in nodes" :key="n.id" :value="n.id">
                {{ n.name }}{{ n.is_self ? ' (this node)' : '' }}
              </option>
            </select>
          </label>

          <label class="col-span-2 flex flex-col gap-1">
            <span class="text-xs text-muted-foreground">Name</span>
            <input v-model="form.name" :class="fieldClass" placeholder="e.g. Camera 1 Sim" />
          </label>

          <label class="col-span-2 flex flex-col gap-1">
            <span class="text-xs text-muted-foreground">Video pattern</span>
            <select v-model="form.pattern" :class="fieldClass">
              <option v-for="p in VIDEO_PATTERNS" :key="p.value" :value="p.value">{{ p.label }}</option>
            </select>
          </label>

          <label class="flex flex-col gap-1">
            <span class="text-xs text-muted-foreground">Resolution</span>
            <select v-model="resolutionKey" :class="fieldClass">
              <option v-for="r in RESOLUTIONS" :key="`${r.w}x${r.h}`" :value="`${r.w}x${r.h}`">
                {{ r.label }} ({{ r.w }}×{{ r.h }})
              </option>
            </select>
          </label>

          <label class="flex flex-col gap-1">
            <span class="text-xs text-muted-foreground">Framerate</span>
            <select v-model="framerateKey" :class="fieldClass">
              <option v-for="r in FRAMERATES" :key="`${r.n}/${r.d}`" :value="`${r.n}/${r.d}`">
                {{ r.label }}
              </option>
            </select>
          </label>

          <label class="flex flex-col gap-1">
            <span class="text-xs text-muted-foreground">Audio signal</span>
            <select v-model="form.audio_signal" :class="fieldClass">
              <option v-for="a in AUDIO_SIGNALS" :key="a.value" :value="a.value">{{ a.label }}</option>
            </select>
          </label>

          <label class="flex flex-col gap-1">
            <span class="text-xs text-muted-foreground">
              Frequency (Hz)
              <span v-if="form.audio_signal !== 'tone'" class="opacity-40">— n/a</span>
            </span>
            <input
              v-model.number="form.frequency"
              type="number"
              :class="fieldClass"
              :disabled="form.audio_signal !== 'tone'"
              placeholder="440"
              min="20"
              max="20000"
            />
          </label>

          <label class="flex flex-col gap-1">
            <span class="text-xs text-muted-foreground">Audio channels</span>
            <select v-model.number="form.channels" :class="fieldClass">
              <option :value="1">Mono</option>
              <option :value="2">Stereo</option>
              <option :value="6">5.1</option>
              <option :value="8">7.1</option>
            </select>
          </label>
        </div>

        <p v-if="formError" class="text-xs text-destructive mt-3">{{ formError }}</p>

        <div class="flex justify-end gap-2 mt-5">
          <Button variant="outline" size="default" :disabled="saving" @click="closeForm">
            Cancel
          </Button>
          <Button size="default" :disabled="saving" @click="save">
            {{ saving ? 'Saving…' : 'Save' }}
          </Button>
        </div>
      </div>
    </div>
  </div>
</template>
