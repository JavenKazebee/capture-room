<script setup lang="ts">
import { computed, ref } from 'vue'
import { ArrowDown, ArrowUp, ChevronRight, Copy, Trash2 } from '@lucide/vue'
import type { Container } from '@/types/generated/Container'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'
import type { VideoCodec } from '@/types/generated/VideoCodec'
import {
  CHROMA_OPTIONS,
  CODECS,
  CODEC_OPTIONS,
  CONTAINERS,
  CONTAINERS_FOR,
  FRAMERATE_PRESETS,
  RESOLUTION_PRESETS,
  containerOptions,
  framerateChoice,
  hasBitrate,
  hasChroma,
  legProblems,
  legSummary,
  parseResolution,
} from '@/lib/codecs'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Input } from '@/components/ui/input'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import FormField from '@/components/FormField.vue'
import { FIELD_HELP } from '@/lib/fieldHelp'
import OptionSelect from '@/components/OptionSelect.vue'
import AdvancedSettings from '@/components/presets/AdvancedSettings.vue'
import PathTemplateField from '@/components/presets/PathTemplateField.vue'
import { advancedProblems, fitAdvanced } from '@/lib/advanced'
import type { ChromaSubsampling } from '@/types/generated/ChromaSubsampling'

const props = defineProps<{
  index: number
  count: number
  /** Another output would write the same file. */
  clash: boolean
  /** Example values for the path preview. */
  preview: { source: string; sourceName: string; node: string; preset: string }
}>()
const leg = defineModel<PresetOutputInput>({ required: true })
defineEmits<{ remove: []; duplicate: []; move: [dir: -1 | 1] }>()

const open = ref(true)
const problems = computed(() => legProblems(leg.value))
const hasProblem = computed(
  () => props.clash || Object.keys(problems.value).length > 0 || Object.keys(advancedProblems(leg.value)).length > 0,
)

/** Apply a change; Advanced choices it made impossible fall back to Auto. */
function patch(p: Partial<PresetOutputInput>) {
  const next = { ...leg.value, ...p }
  leg.value = { ...next, advanced: fitAdvanced(next) }
}

// ── Codec / container ─────────────────────────────────────────────────────────

/** Set when changing codec moved the output to another container, saying why. */
const containerNote = ref<string | null>(null)

/** Switching codec moves the output to a container that codec can record to. */
function setCodec(codec: VideoCodec) {
  const allowed = CONTAINERS_FOR[codec]
  const from = leg.value.container
  const container = allowed.includes(from) ? from : allowed[0]!
  containerNote.value =
    container !== from && allowed.length > 1 ? `Changed from ${CONTAINERS[from]}, which ${CODECS[codec]} can't use` : null
  patch({ codec, container })
}

const container = computed({
  get: () => leg.value.container,
  set: (c: Container) => {
    containerNote.value = null
    patch({ container: c })
  },
})
const onlyContainer = computed(() => {
  const allowed = CONTAINERS_FOR[leg.value.codec]
  return allowed.length === 1 ? allowed[0]! : null
})

const bitrate = computed({
  get: () => leg.value.bitrate_kbps ?? '',
  // An emptied number input yields ""; the server reads null as "encoder default".
  set: (v: string | number) => patch({ bitrate_kbps: v === '' ? null : Number(v) }),
})

// ── Resolution / frame rate ───────────────────────────────────────────────────
// A select of common values plus "Match source" (stored as null) and
// "Custom…", which reveals free entry. A stored value that isn't one of the
// presets opens in Custom.

const SOURCE = 'source'
const CUSTOM = 'custom'

function resolutionChoice(value: string) {
  const r = parseResolution(value)
  return r ? RESOLUTION_PRESETS.find((p) => p.value === `${r[0]}x${r[1]}`)?.value : undefined
}

const resolutionOptions = [
  { value: SOURCE, label: 'Match source' },
  ...RESOLUTION_PRESETS,
  { value: CUSTOM, label: 'Custom…' },
]
const resolutionCustom = ref(!!leg.value.resolution && !resolutionChoice(leg.value.resolution))
const resolutionSelect = computed({
  get: () =>
    resolutionCustom.value ? CUSTOM : leg.value.resolution ? (resolutionChoice(leg.value.resolution) ?? CUSTOM) : SOURCE,
  set: (v: string) => {
    resolutionCustom.value = v === CUSTOM
    if (v === SOURCE) patch({ resolution: null })
    else if (v !== CUSTOM) patch({ resolution: v })
  },
})
/** Width and height of a custom resolution, kept as one WIDTHxHEIGHT string. */
const dims = computed(() => {
  const [w = '', h = ''] = (leg.value.resolution ?? '').split(/x/i)
  return [w.trim(), h.trim()] as const
})
function setDim(i: 0 | 1, v: string | number | undefined) {
  const next = [...dims.value]
  next[i] = String(v ?? '').trim()
  patch({ resolution: next[0] || next[1] ? `${next[0]}x${next[1]}` : null })
}

const framerateOptions = [
  { value: SOURCE, label: 'Match source' },
  ...FRAMERATE_PRESETS,
  { value: CUSTOM, label: 'Custom…' },
]
const framerateCustom = ref(!!leg.value.framerate && !framerateChoice(leg.value.framerate))
const framerateSelect = computed({
  get: () =>
    framerateCustom.value ? CUSTOM : leg.value.framerate ? (framerateChoice(leg.value.framerate) ?? CUSTOM) : SOURCE,
  set: (v: string) => {
    framerateCustom.value = v === CUSTOM
    if (v === SOURCE) patch({ framerate: null })
    else if (v !== CUSTOM) patch({ framerate: v })
  },
})
const framerate = computed({
  get: () => leg.value.framerate ?? '',
  set: (v: string | number) => patch({ framerate: String(v).trim() ? String(v) : null }),
})

</script>

<template>
  <Collapsible v-model:open="open" class="rounded-lg border bg-card" :class="hasProblem ? 'border-destructive/50' : 'border-border'">
    <!-- Header -->
    <div class="flex items-center gap-2 pl-2 pr-1.5 h-10">
      <CollapsibleTrigger class="flex items-center gap-2 flex-1 min-w-0 text-left h-full">
        <ChevronRight class="size-4 shrink-0 text-muted-foreground transition-transform" :class="open && 'rotate-90'" />
        <span class="num text-[11px] text-muted-foreground">{{ index + 1 }}</span>
        <span class="text-sm font-medium truncate">{{ leg.name || `Output ${index + 1}` }}</span>
        <span class="text-xs text-muted-foreground truncate">{{ legSummary(leg) }}</span>
        <span v-if="hasProblem" class="text-xs text-destructive shrink-0">needs attention</span>
      </CollapsibleTrigger>
      <div class="flex items-center shrink-0">
        <Tooltip>
          <TooltipTrigger as-child>
            <button class="card-btn" :disabled="index === 0" @click="$emit('move', -1)"><ArrowUp class="size-3.5" /></button>
          </TooltipTrigger>
          <TooltipContent>Move up</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger as-child>
            <button class="card-btn" :disabled="index === count - 1" @click="$emit('move', 1)"><ArrowDown class="size-3.5" /></button>
          </TooltipTrigger>
          <TooltipContent>Move down</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger as-child>
            <button class="card-btn" @click="$emit('duplicate')"><Copy class="size-3.5" /></button>
          </TooltipTrigger>
          <TooltipContent>Duplicate output</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger as-child>
            <button class="card-btn hover:text-destructive! hover:bg-destructive/10!" :disabled="count === 1" @click="$emit('remove')">
              <Trash2 class="size-3.5" />
            </button>
          </TooltipTrigger>
          <TooltipContent>{{ count === 1 ? 'A preset needs at least one output' : 'Remove output' }}</TooltipContent>
        </Tooltip>
      </div>
    </div>

    <CollapsibleContent>
      <div class="grid grid-cols-2 lg:grid-cols-4 gap-3 px-3 pb-3 pt-1 border-t border-border [&>*]:min-w-0">
        <FormField label="Output name" :help="FIELD_HELP.outputName" class="col-span-2">
          <Input v-model="leg.name" placeholder="Primary" />
        </FormField>
        <FormField label="Codec" :help="FIELD_HELP.codec">
          <OptionSelect :model-value="leg.codec" :options="CODEC_OPTIONS" @update:model-value="setCodec" />
        </FormField>
        <FormField label="Container" :help="FIELD_HELP.container">
          <span v-if="onlyContainer" class="h-7 flex items-center text-xs">
            {{ CONTAINERS[onlyContainer] }}
            <span class="ml-1.5 text-muted-foreground">· only option for {{ CODECS[leg.codec] }}</span>
          </span>
          <OptionSelect v-else v-model="container" :options="containerOptions(leg.codec)" />
          <span v-if="containerNote" class="text-[11px] text-muted-foreground">{{ containerNote }}</span>
        </FormField>

        <FormField label="Resolution" :help="FIELD_HELP.resolution">
          <OptionSelect v-model="resolutionSelect" :options="resolutionOptions" />
          <div v-if="resolutionSelect === CUSTOM" class="flex items-center gap-1.5">
            <Input
              :model-value="dims[0]"
              inputmode="numeric"
              placeholder="W"
              aria-label="Width"
              class="num h-7 flex-1 min-w-0 px-2"
              :aria-invalid="!!problems.resolution"
              @update:model-value="(v) => setDim(0, v)"
            />
            <span class="text-xs text-muted-foreground">×</span>
            <Input
              :model-value="dims[1]"
              inputmode="numeric"
              placeholder="H"
              aria-label="Height"
              class="num h-7 flex-1 min-w-0 px-2"
              :aria-invalid="!!problems.resolution"
              @update:model-value="(v) => setDim(1, v)"
            />
          </div>
          <span v-if="problems.resolution" class="text-[11px] text-destructive">{{ problems.resolution }}</span>
        </FormField>
        <FormField label="Frame rate" :help="FIELD_HELP.framerate">
          <OptionSelect v-model="framerateSelect" :options="framerateOptions" />
          <Input
            v-if="framerateSelect === CUSTOM"
            v-model="framerate"
            placeholder="e.g. 12.5 or 30000/1001"
            aria-label="Custom frame rate"
            class="num h-7"
            :aria-invalid="!!problems.framerate"
          />
          <span v-if="problems.framerate" class="text-[11px] text-destructive">{{ problems.framerate }}</span>
        </FormField>
        <FormField v-if="hasBitrate(leg.codec) && leg.advanced.rate_control === 'quality'" label="Bitrate" :help="FIELD_HELP.bitrateQuality">
          <span class="h-7 flex items-center text-xs text-muted-foreground">Set by quality</span>
        </FormField>
        <FormField v-else-if="hasBitrate(leg.codec)" label="Bitrate (kbps)" :help="FIELD_HELP.bitrate">
          <Input v-model="bitrate" type="number" min="0" placeholder="Auto" class="num" />
        </FormField>
        <FormField v-else label="Bitrate" :help="FIELD_HELP.bitrateFixed">
          <span class="h-7 flex items-center text-xs text-muted-foreground">Set by the codec</span>
        </FormField>
        <FormField v-if="hasChroma(leg.codec)" label="Chroma" :help="FIELD_HELP.chroma">
          <OptionSelect :model-value="leg.chroma" :options="CHROMA_OPTIONS" @update:model-value="(c: ChromaSubsampling) => patch({ chroma: c })" />
        </FormField>
        <FormField v-else label="Chroma" :help="FIELD_HELP.chromaFixed">
          <span class="h-7 flex items-center text-xs text-muted-foreground">Set by the codec</span>
        </FormField>

        <PathTemplateField
          v-model="leg"
          class="col-span-2 lg:col-span-4"
          :clash="clash"
          :problem="problems.path"
          :multiple-outputs="count > 1"
          :preview="preview"
        />

        <AdvancedSettings v-model="leg" />
      </div>
    </CollapsibleContent>
  </Collapsible>
</template>

<style scoped>
@reference "@/style.css";
.card-btn {
  @apply size-7 grid place-items-center rounded text-muted-foreground hover:text-foreground hover:bg-accent disabled:opacity-30 disabled:pointer-events-none;
}
</style>
