<script setup lang="ts">
import { computed, nextTick, ref } from 'vue'
import { ArrowDown, ArrowUp, ChevronRight, Copy, Trash2 } from '@lucide/vue'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'
import type { VideoCodec } from '@/types/generated/VideoCodec'
import {
  CHROMA_OPTIONS,
  CODEC_OPTIONS,
  CONTAINERS_FOR,
  PATH_TOKENS,
  containerOptions,
  expandPath,
  hasChroma,
  legProblems,
  legSummary,
} from '@/lib/codecs'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Input } from '@/components/ui/input'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'

const props = defineProps<{
  index: number
  count: number
  /** Another output would write the same file. */
  clash: boolean
  /** Example values for the path preview. */
  preview: { source: string; node: string }
}>()
const leg = defineModel<PresetOutputInput>({ required: true })
defineEmits<{ remove: []; duplicate: []; move: [dir: -1 | 1] }>()

const open = ref(true)
const problems = computed(() => legProblems(leg.value))
const hasProblem = computed(() => props.clash || Object.keys(problems.value).length > 0)

/** Switching codec moves the output to a container that codec can record to. */
function setCodec(codec: VideoCodec) {
  const allowed = CONTAINERS_FOR[codec]
  leg.value = { ...leg.value, codec, container: allowed.includes(leg.value.container) ? leg.value.container : allowed[0]! }
}

const bitrate = computed({
  get: () => leg.value.bitrate_kbps ?? '',
  // An emptied number input yields ""; the server reads null as "encoder default".
  set: (v: string | number) => (leg.value = { ...leg.value, bitrate_kbps: v === '' ? null : Number(v) }),
})

const optional = (key: 'resolution' | 'framerate') =>
  computed({
    get: () => leg.value[key] ?? '',
    set: (v: string | number) => (leg.value = { ...leg.value, [key]: String(v).trim() ? String(v) : null }),
  })
const resolution = optional('resolution')
const framerate = optional('framerate')

// ── Path template ────────────────────────────────────────────────────────────

const pathInput = ref<{ $el: HTMLInputElement } | null>(null)

/** Insert a token at the cursor (or the end) and keep the cursor after it. */
async function insertToken(token: string) {
  const el = (pathInput.value?.$el as HTMLInputElement | undefined) ?? null
  const t = leg.value.path_template
  const start = el?.selectionStart ?? t.length
  const end = el?.selectionEnd ?? t.length
  leg.value = { ...leg.value, path_template: t.slice(0, start) + token + t.slice(end) }
  await nextTick()
  el?.focus()
  el?.setSelectionRange(start + token.length, start + token.length)
}

const previewPath = computed(() => expandPath(leg.value, props.preview))
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
        <FormField label="Output name" class="col-span-2">
          <Input v-model="leg.name" placeholder="Primary" />
        </FormField>
        <FormField label="Codec">
          <OptionSelect :model-value="leg.codec" :options="CODEC_OPTIONS" @update:model-value="setCodec" />
        </FormField>
        <FormField label="Container">
          <OptionSelect v-model="leg.container" :options="containerOptions(leg.codec)" />
        </FormField>

        <FormField label="Resolution" title="WIDTHxHEIGHT; leave blank to keep the source's">
          <Input v-model="resolution" placeholder="source" class="num" :aria-invalid="!!problems.resolution" />
          <span v-if="problems.resolution" class="text-[11px] text-destructive">{{ problems.resolution }}</span>
        </FormField>
        <FormField label="Frame rate" title="30, 25, 30000/1001…; leave blank to keep the source's">
          <Input v-model="framerate" placeholder="source" class="num" :aria-invalid="!!problems.framerate" />
          <span v-if="problems.framerate" class="text-[11px] text-destructive">{{ problems.framerate }}</span>
        </FormField>
        <FormField label="Bitrate (kbps)" title="Leave blank for the encoder's default">
          <Input v-model="bitrate" type="number" min="0" placeholder="encoder default" class="num" />
        </FormField>
        <FormField v-if="hasChroma(leg.codec)" label="Chroma">
          <OptionSelect v-model="leg.chroma" :options="CHROMA_OPTIONS" />
        </FormField>
        <div v-else class="flex flex-col gap-1">
          <span class="text-xs text-muted-foreground">Chroma</span>
          <span class="h-7 flex items-center text-xs text-muted-foreground">Set by the codec</span>
        </div>

        <!-- Path -->
        <div class="col-span-2 lg:col-span-4 flex flex-col gap-1.5">
          <span class="text-xs text-muted-foreground">Path template <span class="opacity-60">· ~ is the recording node's home</span></span>
          <Input ref="pathInput" v-model="leg.path_template" class="num" :aria-invalid="!!problems.path || clash" />
          <div class="flex flex-wrap items-center gap-1">
            <Tooltip v-for="t in PATH_TOKENS" :key="t.token">
              <TooltipTrigger as-child>
                <button
                  class="num text-[11px] rounded border border-border px-1.5 py-0.5 text-muted-foreground hover:text-primary hover:border-primary/50"
                  @click="insertToken(t.token)"
                >
                  {{ t.token }}
                </button>
              </TooltipTrigger>
              <TooltipContent>{{ t.help }} — click to insert</TooltipContent>
            </Tooltip>
          </div>
          <div class="text-xs flex gap-2 min-w-0">
            <span class="text-muted-foreground shrink-0">Preview</span>
            <span class="num break-all">{{ previewPath }}</span>
          </div>
          <span v-if="problems.path" class="text-[11px] text-destructive">{{ problems.path }}</span>
          <span v-if="clash" class="text-[11px] text-destructive">
            Another output writes the same file — vary the path, container, or include {output}.
          </span>
        </div>
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
