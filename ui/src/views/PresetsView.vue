<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { usePresetsStore, blankLeg } from '@/stores/presets'
import type { PresetDto } from '@/types/generated/PresetDto'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import type { ChromaSubsampling } from '@/types/generated/ChromaSubsampling'
import type { Container } from '@/types/generated/Container'
import type { VideoCodec } from '@/types/generated/VideoCodec'

const store = usePresetsStore()

const editingId = ref<string | null>(null)
const showForm = ref(false)
const saving = ref(false)
const error = ref<string | null>(null)

// Records keyed by the generated unions, so a codec or container added in Rust
// fails the type check here until it gets a label.
const CODECS: Record<VideoCodec, string> = {
  h264: 'H.264',
  h265: 'H.265 / HEVC',
  vp9: 'VP9',
  prores_4444: 'ProRes 4444',
  prores_422hq: 'ProRes 422 HQ',
  prores_422: 'ProRes 422',
  prores_422lt: 'ProRes 422 LT',
  prores_422proxy: 'ProRes 422 Proxy',
  uncompressed: 'Uncompressed',
}
const CONTAINERS: Record<Container, string> = { mov: '.mov', mp4: '.mp4', mkv: '.mkv', mxf: '.mxf' }
const CHROMA: { value: ChromaSubsampling; label: string }[] = [
  { value: '420', label: '4:2:0 — plays everywhere' },
  { value: '422', label: '4:2:2' },
  { value: '444', label: '4:4:4' },
]

/** Codecs whose chroma subsampling is configurable (ProRes picks it via the codec). */
function hasChroma(codec: VideoCodec) {
  return codec === 'h264' || codec === 'h265'
}

function chromaLabel(chroma: ChromaSubsampling) {
  return `${chroma[0]}:${chroma[1]}:${chroma[2]}`
}

const fieldClass =
  'h-8 rounded-md border border-border bg-background px-2 text-sm outline-none focus:ring-2 focus:ring-ring/30'

const formName = ref('')
const formLegs = ref<PresetOutputInput[]>([blankLeg()])

function blankToNull(v: string | null | undefined): string | null {
  return v && String(v).trim() !== '' ? String(v) : null
}

function normalizedLegs(): PresetOutputInput[] {
  return formLegs.value.map((leg) => ({
    ...leg,
    resolution: blankToNull(leg.resolution),
    framerate: blankToNull(leg.framerate),
    bitrate_kbps: leg.bitrate_kbps ? Number(leg.bitrate_kbps) : null,
  }))
}

function openCreate() {
  editingId.value = null
  formName.value = ''
  formLegs.value = [blankLeg()]
  error.value = null
  showForm.value = true
}

function openEdit(p: PresetDto) {
  editingId.value = p.id
  formName.value = p.name
  formLegs.value = p.outputs.map((o) => ({
    name: o.name,
    codec: o.codec,
    container: o.container,
    resolution: o.resolution,
    framerate: o.framerate,
    bitrate_kbps: o.bitrate_kbps,
    chroma: o.chroma,
    path_template: o.path_template,
  }))
  if (formLegs.value.length === 0) formLegs.value = [blankLeg()]
  error.value = null
  showForm.value = true
}

function closeForm() {
  showForm.value = false
}

function addLeg() {
  formLegs.value.push(blankLeg())
}

function removeLeg(i: number) {
  formLegs.value.splice(i, 1)
}

async function save() {
  if (saving.value) return
  if (!formName.value.trim()) {
    error.value = 'Name is required.'
    return
  }
  if (formLegs.value.length === 0) {
    error.value = 'At least one output leg is required.'
    return
  }
  saving.value = true
  error.value = null
  try {
    const payload = { name: formName.value.trim(), outputs: normalizedLegs() }
    if (editingId.value) await store.update(editingId.value, payload)
    else await store.create(payload)
    showForm.value = false
  } catch (e) {
    error.value = e instanceof Error ? e.message : 'Save failed.'
  } finally {
    saving.value = false
  }
}

async function destroy(p: PresetDto) {
  if (!confirm(`Delete preset "${p.name}"?`)) return
  try {
    await store.remove(p.id)
  } catch (e) {
    error.value = e instanceof Error ? e.message : 'Delete failed.'
  }
}

onMounted(() => store.load())
</script>

<template>
  <div class="p-6 max-w-4xl">
    <div class="flex items-center justify-between mb-6">
      <h1 class="text-2xl font-semibold">Presets</h1>
      <Button size="default" @click="openCreate">New preset</Button>
    </div>

    <!-- Empty state -->
    <div
      v-if="store.presets.length === 0"
      class="text-center text-muted-foreground py-16 rounded-lg border border-dashed border-border"
    >
      No presets yet. Create one to configure recording output.
    </div>

    <!-- Preset list -->
    <div v-else class="rounded-lg border border-border bg-card divide-y divide-border">
      <div v-for="p in store.presets" :key="p.id" class="px-4 py-3">
        <div class="flex items-start gap-3">
          <div class="flex-1 min-w-0">
            <span class="text-sm font-medium">{{ p.name }}</span>
            <!-- Per-leg summary -->
            <div
              v-for="(leg, i) in p.outputs"
              :key="i"
              class="flex items-center gap-2 mt-1 text-xs text-muted-foreground"
            >
              <Badge variant="secondary" class="text-xs">{{ CODECS[leg.codec] }}</Badge>
              <Badge variant="outline" class="text-xs">{{ CONTAINERS[leg.container] }}</Badge>
              <span>{{ leg.resolution ?? 'source res' }} · {{ leg.framerate ?? 'source fps' }}</span>
              <span>·</span>
              <span>{{ leg.bitrate_kbps ? `${leg.bitrate_kbps} kbps` : 'encoder default' }}</span>
              <template v-if="hasChroma(leg.codec)">
                <span>·</span>
                <span>{{ chromaLabel(leg.chroma) }}</span>
              </template>
              <span class="font-mono truncate">{{ leg.path_template }}</span>
            </div>
            <p v-if="p.outputs.length === 0" class="text-xs text-muted-foreground mt-1 italic">
              No output legs configured.
            </p>
          </div>
          <div class="flex gap-2 shrink-0">
            <Button variant="outline" size="default" @click="openEdit(p)">Edit</Button>
            <Button variant="destructive" size="default" @click="destroy(p)">Delete</Button>
          </div>
        </div>
      </div>
    </div>

    <!-- Create / edit form -->
    <div
      v-if="showForm"
      class="fixed inset-0 bg-black/40 flex items-center justify-center p-4 z-50"
      @click.self="closeForm"
    >
      <div class="bg-card border border-border rounded-lg w-full max-w-2xl max-h-[90vh] overflow-y-auto p-5">
        <h2 class="text-lg font-semibold mb-4">{{ editingId ? 'Edit preset' : 'New preset' }}</h2>

        <!-- Preset name -->
        <label class="flex flex-col gap-1 mb-5">
          <span class="text-xs text-muted-foreground">Preset name</span>
          <input v-model="formName" :class="fieldClass" placeholder="e.g. Broadcast H.264" />
        </label>

        <!-- Output legs -->
        <div class="flex items-center justify-between mb-2">
          <span class="text-sm font-medium">Output legs</span>
          <Button variant="outline" size="sm" @click="addLeg">+ Add leg</Button>
        </div>

        <div class="space-y-4">
          <div
            v-for="(leg, i) in formLegs"
            :key="i"
            class="rounded-md border border-border p-3 relative"
          >
            <!-- Leg header -->
            <div class="flex items-center justify-between mb-3">
              <span class="text-xs font-semibold text-muted-foreground uppercase tracking-wide">
                Leg {{ i + 1 }}
              </span>
              <button
                v-if="formLegs.length > 1"
                class="text-xs text-destructive hover:underline"
                @click="removeLeg(i)"
              >
                Remove
              </button>
            </div>

            <div class="grid grid-cols-2 gap-3">
              <label class="col-span-2 flex flex-col gap-1">
                <span class="text-xs text-muted-foreground">Leg name</span>
                <input v-model="leg.name" :class="fieldClass" placeholder="e.g. Primary H.264" />
              </label>

              <label class="flex flex-col gap-1">
                <span class="text-xs text-muted-foreground">Codec</span>
                <select v-model="leg.codec" :class="fieldClass">
                  <option v-for="(label, c) in CODECS" :key="c" :value="c">{{ label }}</option>
                </select>
              </label>

              <label class="flex flex-col gap-1">
                <span class="text-xs text-muted-foreground">Container</span>
                <select v-model="leg.container" :class="fieldClass">
                  <option v-for="(label, c) in CONTAINERS" :key="c" :value="c">{{ label }}</option>
                </select>
              </label>

              <label class="flex flex-col gap-1">
                <span class="text-xs text-muted-foreground">Resolution</span>
                <input v-model="leg.resolution" :class="fieldClass" placeholder="match source / 1920x1080" />
              </label>

              <label class="flex flex-col gap-1">
                <span class="text-xs text-muted-foreground">Framerate</span>
                <input v-model="leg.framerate" :class="fieldClass" placeholder="source / 30 / 30000/1001" />
              </label>

              <label class="flex flex-col gap-1">
                <span class="text-xs text-muted-foreground">Bitrate (kbps)</span>
                <input v-model.number="leg.bitrate_kbps" type="number" :class="fieldClass" placeholder="8000" />
              </label>

              <label v-if="hasChroma(leg.codec)" class="flex flex-col gap-1">
                <span class="text-xs text-muted-foreground">Chroma</span>
                <select v-model="leg.chroma" :class="fieldClass">
                  <option v-for="c in CHROMA" :key="c.value" :value="c.value">{{ c.label }}</option>
                </select>
              </label>

              <label class="col-span-2 flex flex-col gap-1">
                <span class="text-xs text-muted-foreground">
                  Path template
                  <span class="text-muted-foreground/60 ml-1">{source} {datetime} {ext}</span>
                </span>
                <input v-model="leg.path_template" :class="[fieldClass, 'font-mono']" />
              </label>
            </div>
          </div>
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
