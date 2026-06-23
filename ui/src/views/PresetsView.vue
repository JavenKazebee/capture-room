<script setup lang="ts">
import { onMounted, reactive, ref } from 'vue'
import { useApi } from '@/composables/useApi'
import { usePresetsStore, type Preset, type PresetInput } from '@/stores/presets'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'

const { api } = useApi()
const store = usePresetsStore()

const isAggregator = ref(false)
const editingId = ref<string | null>(null)
const showForm = ref(false)
const saving = ref(false)
const error = ref<string | null>(null)

const CODECS = ['h264', 'h265', 'vp9', 'prores', 'dnxhd', 'uncompressed']
const CONTAINERS = ['mov', 'mp4', 'mkv', 'mxf']

const fieldClass =
  'h-8 rounded-md border border-border bg-background px-2 text-sm outline-none focus:ring-2 focus:ring-ring/30'

function blankForm(): PresetInput {
  return {
    name: '',
    codec: 'h264',
    container: 'mov',
    resolution: null,
    framerate: null,
    bitrate_kbps: 8000,
    quality: null,
    output_template: '/tmp/capture-room/{source}_{datetime}.{ext}',
    secondary_output_template: null,
    redundant_output_template: null,
  }
}

const form = reactive<PresetInput>(blankForm())

function stripMeta(p: Preset): PresetInput {
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  const { id, created_at, updated_at, version, ...rest } = p
  return rest
}

function resetForm(p?: Preset) {
  Object.assign(form, p ? { ...blankForm(), ...stripMeta(p) } : blankForm())
}

function openCreate() {
  editingId.value = null
  resetForm()
  error.value = null
  showForm.value = true
}

function openEdit(p: Preset) {
  editingId.value = p.id
  resetForm(p)
  error.value = null
  showForm.value = true
}

function closeForm() {
  showForm.value = false
}

// Empty strings in optional fields should be sent as null.
function normalized(): PresetInput {
  const blankToNull = (v: string | null) => (v && String(v).trim() !== '' ? v : null)
  return {
    ...form,
    resolution: blankToNull(form.resolution),
    framerate: blankToNull(form.framerate),
    quality: blankToNull(form.quality),
    secondary_output_template: blankToNull(form.secondary_output_template),
    redundant_output_template: blankToNull(form.redundant_output_template),
    bitrate_kbps: form.bitrate_kbps ? Number(form.bitrate_kbps) : null,
  }
}

async function save() {
  if (saving.value) return
  if (!form.name.trim()) {
    error.value = 'Name is required.'
    return
  }
  saving.value = true
  error.value = null
  try {
    if (editingId.value) await store.update(editingId.value, normalized())
    else await store.create(normalized())
    showForm.value = false
  } catch (e) {
    error.value = e instanceof Error ? e.message : 'Save failed.'
  } finally {
    saving.value = false
  }
}

async function destroy(p: Preset) {
  if (!confirm(`Delete preset "${p.name}"?`)) return
  try {
    await store.remove(p.id)
  } catch (e) {
    error.value = e instanceof Error ? e.message : 'Delete failed.'
  }
}

onMounted(async () => {
  const settings = await api<{ role: string }>('/settings').catch(() => null)
  isAggregator.value = settings?.role === 'aggregator'
  await store.load()
})
</script>

<template>
  <div class="p-6 max-w-4xl">
    <div class="flex items-center justify-between mb-6">
      <h1 class="text-2xl font-semibold">Presets</h1>
      <Button v-if="isAggregator" size="default" @click="openCreate">New preset</Button>
    </div>

    <p v-if="!isAggregator" class="text-sm text-muted-foreground mb-4">
      Presets are managed on the control station. This machine shows the synced set read-only.
    </p>

    <!-- Empty state -->
    <div
      v-if="store.presets.length === 0"
      class="text-center text-muted-foreground py-16 rounded-lg border border-dashed border-border"
    >
      No presets yet.<span v-if="isAggregator"> Create one to configure recording output.</span>
    </div>

    <!-- List -->
    <div v-else class="rounded-lg border border-border bg-card divide-y divide-border">
      <div v-for="p in store.presets" :key="p.id" class="flex items-center gap-3 px-4 py-3">
        <div class="flex-1 min-w-0">
          <div class="flex items-center gap-2">
            <span class="text-sm font-medium truncate">{{ p.name }}</span>
            <Badge variant="secondary">{{ p.codec }}</Badge>
            <Badge variant="outline">.{{ p.container }}</Badge>
          </div>
          <div class="text-xs text-muted-foreground truncate mt-0.5">
            {{ p.resolution ?? 'match source' }} ·
            {{ p.framerate ?? 'source fps' }} ·
            {{ p.bitrate_kbps ? `${p.bitrate_kbps} kbps` : (p.quality ?? 'quality-based') }} ·
            <span class="font-mono">{{ p.output_template }}</span>
          </div>
        </div>
        <div v-if="isAggregator" class="flex gap-2 shrink-0">
          <Button variant="outline" size="default" @click="openEdit(p)">Edit</Button>
          <Button variant="destructive" size="default" @click="destroy(p)">Delete</Button>
        </div>
      </div>
    </div>

    <!-- Create / edit form -->
    <div
      v-if="showForm"
      class="fixed inset-0 bg-black/40 flex items-center justify-center p-4 z-50"
      @click.self="closeForm"
    >
      <div class="bg-card border border-border rounded-lg w-full max-w-lg max-h-[90vh] overflow-y-auto p-5">
        <h2 class="text-lg font-semibold mb-4">{{ editingId ? 'Edit preset' : 'New preset' }}</h2>

        <div class="grid grid-cols-2 gap-3">
          <label class="col-span-2 flex flex-col gap-1">
            <span class="text-xs text-muted-foreground">Name</span>
            <input v-model="form.name" :class="fieldClass" placeholder="e.g. Broadcast H.264" />
          </label>

          <label class="flex flex-col gap-1">
            <span class="text-xs text-muted-foreground">Codec</span>
            <select v-model="form.codec" :class="fieldClass">
              <option v-for="c in CODECS" :key="c" :value="c">{{ c }}</option>
            </select>
          </label>

          <label class="flex flex-col gap-1">
            <span class="text-xs text-muted-foreground">Container</span>
            <select v-model="form.container" :class="fieldClass">
              <option v-for="c in CONTAINERS" :key="c" :value="c">.{{ c }}</option>
            </select>
          </label>

          <label class="flex flex-col gap-1">
            <span class="text-xs text-muted-foreground">Resolution</span>
            <input v-model="form.resolution" :class="fieldClass" placeholder="match source / 1920x1080" />
          </label>

          <label class="flex flex-col gap-1">
            <span class="text-xs text-muted-foreground">Framerate</span>
            <input v-model="form.framerate" :class="fieldClass" placeholder="source / 30 / 30000/1001" />
          </label>

          <label class="flex flex-col gap-1">
            <span class="text-xs text-muted-foreground">Bitrate (kbps)</span>
            <input v-model.number="form.bitrate_kbps" type="number" :class="fieldClass" placeholder="8000" />
          </label>

          <label class="flex flex-col gap-1">
            <span class="text-xs text-muted-foreground">Quality</span>
            <input v-model="form.quality" :class="fieldClass" placeholder="optional" />
          </label>

          <label class="col-span-2 flex flex-col gap-1">
            <span class="text-xs text-muted-foreground">Output template</span>
            <input v-model="form.output_template" :class="[fieldClass, 'font-mono']" />
          </label>

          <label class="col-span-2 flex flex-col gap-1">
            <span class="text-xs text-muted-foreground">Secondary output (optional)</span>
            <input v-model="form.secondary_output_template" :class="[fieldClass, 'font-mono']" placeholder="none" />
          </label>

          <label class="col-span-2 flex flex-col gap-1">
            <span class="text-xs text-muted-foreground">Redundant output (optional)</span>
            <input v-model="form.redundant_output_template" :class="[fieldClass, 'font-mono']" placeholder="none" />
          </label>
        </div>

        <p v-if="error" class="text-xs text-destructive mt-3">{{ error }}</p>

        <div class="flex justify-end gap-2 mt-5">
          <Button variant="outline" size="default" :disabled="saving" @click="closeForm">Cancel</Button>
          <Button size="default" :disabled="saving" @click="save">
            {{ saving ? 'Saving…' : 'Save' }}
          </Button>
        </div>
      </div>
    </div>
  </div>
</template>
