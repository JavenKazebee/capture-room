<script setup lang="ts">
import { computed } from 'vue'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'
import type { SlateSettings } from '@/types/generated/SlateSettings'
import { expandPath, expandSlateTitle } from '@/lib/codecs'
import { SLATE_LENGTHS, SLATE_TOKEN_GROUPS, newSlate, slateProblem } from '@/lib/slate'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import FormField from '@/components/FormField.vue'
import TokenInput from '@/components/presets/TokenInput.vue'

/**
 * A preset's slate: how long it covers the start of each take, its title
 * template and its two colours, with a preview of the first output's card.
 */
const props = defineProps<{
  /** The first output, for the preview. */
  leg: PresetOutputInput | undefined
  /** Example values for the preview. */
  preview: { source: string; sourceName: string; node: string; preset: string }
}>()
const slate = defineModel<SlateSettings | null>({ required: true })

const length = computed({
  get: () => slate.value?.length ?? 'off',
  set: (v: string) => {
    if (!v || v === 'off') slate.value = null
    else slate.value = { ...(slate.value ?? newSlate('5s')), length: v as SlateSettings['length'] }
  },
})

function patch(p: Partial<SlateSettings>) {
  if (slate.value) slate.value = { ...slate.value, ...p }
}

const problem = computed(() => slateProblem(slate.value))

const card = computed(() => {
  const s = slate.value
  const leg = props.leg
  if (!s || !leg) return null
  const now = new Date()
  const file = expandPath(leg, props.preview, now).split(/[\\/]/).pop() ?? ''
  const title = s.title.trim()
    ? expandSlateTitle(s.title, leg, props.preview, now)
    : file.replace(/\.[^.]*$/, '')
  const pad = (n: number) => String(n).padStart(2, '0')
  const date = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())} ${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}`
  const lines = [leg.path_template.includes('{take}') ? 'Take 01' : '', date, props.preview.sourceName].filter(Boolean)
  return { title, lines }
})
</script>

<template>
  <section class="rounded-lg border border-border p-3 flex flex-col gap-3">
    <div class="flex flex-wrap items-center gap-3">
      <h2 class="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">Slate</h2>
      <ToggleGroup v-model="length" type="single" variant="segmented" aria-label="Slate length">
        <ToggleGroupItem value="off" class="h-7 px-3 text-xs">Off</ToggleGroupItem>
        <ToggleGroupItem v-for="l in SLATE_LENGTHS" :key="l.value" :value="l.value" class="h-7 px-3 text-xs">
          {{ l.label }}
        </ToggleGroupItem>
      </ToggleGroup>
      <span class="text-[11px] text-muted-foreground">
        A card over the start of every take's picture, however it's started. The sound runs under it.
      </span>
    </div>

    <div v-if="slate" class="flex flex-wrap items-start gap-4">
      <div class="flex flex-col gap-3 flex-1 min-w-64">
        <FormField label="Title">
          <TokenInput
            :model-value="slate.title"
            :groups="SLATE_TOKEN_GROUPS"
            :invalid="!!problem && problem.includes('token')"
            placeholder="The file's name"
            aria-label="Slate title"
            @update:model-value="(title: string) => patch({ title })"
          />
        </FormField>
        <div class="flex gap-4">
          <FormField label="Background">
            <input
              type="color"
              class="h-8 w-14 rounded border border-input bg-transparent cursor-pointer"
              :value="slate.background"
              aria-label="Slate background colour"
              @input="patch({ background: ($event.target as HTMLInputElement).value })"
            />
          </FormField>
          <FormField label="Text">
            <input
              type="color"
              class="h-8 w-14 rounded border border-input bg-transparent cursor-pointer"
              :value="slate.text"
              aria-label="Slate text colour"
              @input="patch({ text: ($event.target as HTMLInputElement).value })"
            />
          </FormField>
        </div>
        <p v-if="problem" class="text-xs text-destructive">{{ problem }}</p>
        <p v-else class="text-[11px] text-muted-foreground">
          Under the title: the take (when the file name counts them), the date and time, and the source.
        </p>
      </div>

      <div
        v-if="card"
        class="w-72 aspect-video rounded border border-border flex flex-col items-center justify-center text-center px-3 overflow-hidden"
        :style="{ background: slate.background, color: slate.text }"
        aria-label="Slate preview"
      >
        <span class="text-sm font-bold leading-tight break-all">{{ card.title }}</span>
        <span v-for="line in card.lines" :key="line" class="text-[10px] leading-snug">{{ line }}</span>
      </div>
    </div>
  </section>
</template>
