<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { usePresetsStore, blankLeg, presetLegs } from '@/stores/presets'
import { errorMessage } from '@/composables/useApi'
import type { PresetDto } from '@/types/generated/PresetDto'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import FormField from '@/components/FormField.vue'
import FormModal from '@/components/FormModal.vue'
import OptionSelect from '@/components/OptionSelect.vue'
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
const CODEC_OPTIONS = optionsOf(CODECS)
const CONTAINER_OPTIONS = optionsOf(CONTAINERS)
const CHROMA: { value: ChromaSubsampling; label: string }[] = [
  { value: '420', label: '4:2:0 — plays everywhere' },
  { value: '422', label: '4:2:2' },
  { value: '444', label: '4:4:4' },
]

function optionsOf<T extends string>(labels: Record<T, string>) {
  return (Object.entries(labels) as [T, string][]).map(([value, label]) => ({ value, label }))
}

/** Codecs whose chroma subsampling is configurable (ProRes picks it via the codec). */
function hasChroma(codec: VideoCodec) {
  return codec === 'h264' || codec === 'h265'
}

function chromaLabel(chroma: ChromaSubsampling) {
  return `${chroma[0]}:${chroma[1]}:${chroma[2]}`
}

const formName = ref('')
const formLegs = ref<PresetOutputInput[]>([blankLeg()])

// The server stores blank resolution/framerate as "match source"; only the
// bitrate needs fixing up, since an emptied number input yields "".
function normalizedLegs(): PresetOutputInput[] {
  return formLegs.value.map((leg) => ({
    ...leg,
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
  formLegs.value = presetLegs(p)
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
    error.value = errorMessage(e, 'Save failed.')
  } finally {
    saving.value = false
  }
}

async function destroy(p: PresetDto) {
  if (!confirm(`Delete preset "${p.name}"?`)) return
  try {
    await store.remove(p.id)
  } catch (e) {
    error.value = errorMessage(e, 'Delete failed.')
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
    <FormModal
      v-if="showForm"
      :title="editingId ? 'Edit preset' : 'New preset'"
      :error="error"
      :saving="saving"
      wide
      @close="closeForm"
      @save="save"
    >
      <FormField label="Preset name" class="mb-5">
        <Input v-model="formName" placeholder="e.g. Broadcast H.264" />
      </FormField>

      <!-- Output legs -->
      <div class="flex items-center justify-between mb-2">
        <span class="text-sm font-medium">Output legs</span>
        <Button variant="outline" size="sm" @click="addLeg">+ Add leg</Button>
      </div>

      <div class="space-y-4">
        <div v-for="(leg, i) in formLegs" :key="i" class="rounded-md border border-border p-3">
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
            <FormField label="Leg name" class="col-span-2">
              <Input v-model="leg.name" placeholder="e.g. Primary H.264" />
            </FormField>

            <FormField label="Codec">
              <OptionSelect v-model="leg.codec" :options="CODEC_OPTIONS" />
            </FormField>

            <FormField label="Container">
              <OptionSelect v-model="leg.container" :options="CONTAINER_OPTIONS" />
            </FormField>

            <FormField label="Resolution">
              <Input v-model="leg.resolution" placeholder="match source / 1920x1080" />
            </FormField>

            <FormField label="Framerate">
              <Input v-model="leg.framerate" placeholder="source / 30 / 30000/1001" />
            </FormField>

            <FormField label="Bitrate (kbps)">
              <Input v-model.number="leg.bitrate_kbps" type="number" placeholder="encoder default" />
            </FormField>

            <FormField v-if="hasChroma(leg.codec)" label="Chroma">
              <OptionSelect v-model="leg.chroma" :options="CHROMA" />
            </FormField>

            <FormField class="col-span-2">
              <template #label>
                Path template
                <span class="text-muted-foreground/60 ml-1">{source} {datetime} {ext}</span>
              </template>
              <Input v-model="leg.path_template" class="font-mono" />
            </FormField>
          </div>
        </div>
      </div>
    </FormModal>
  </div>
</template>
