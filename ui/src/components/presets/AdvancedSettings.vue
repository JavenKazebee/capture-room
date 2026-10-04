<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { ChevronRight, RotateCcw } from '@lucide/vue'
import type { AudioChannels } from '@/types/generated/AudioChannels'
import type { AudioCodecChoice } from '@/types/generated/AudioCodecChoice'
import type { Deinterlace } from '@/types/generated/Deinterlace'
import type { EncoderChoice } from '@/types/generated/EncoderChoice'
import type { OutputAdvanced } from '@/types/generated/OutputAdvanced'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'
import type { RateControl } from '@/types/generated/RateControl'
import type { SpeedPreset } from '@/types/generated/SpeedPreset'
import { useNodesStore } from '@/stores/nodes'
import { hasBitrate } from '@/lib/codecs'
import {
  AUDIO_DEFAULT_KBPS,
  DEFAULT_KEYFRAME_SECS,
  DEFAULT_QUALITY,
  DEFAULT_SPEED,
  RATE_CONTROL_OPTIONS,
  SPEED_OPTIONS,
  advancedProblems,
  advancedSummary,
  audioCodecOptions,
  defaultAdvanced,
  encoderChoiceProblem,
  encoderOnNodes,
  formatChannels,
  isProRes,
  parseChannels,
  resolvedAudio,
  usesSpeedPreset,
} from '@/lib/advanced'
import { FIELD_HELP } from '@/lib/fieldHelp'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Input } from '@/components/ui/input'
import { Slider } from '@/components/ui/slider'
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'

/**
 * An output's Advanced settings: collapsed while everything is default, with
 * a summary of whatever isn't, so a changed setting is never hidden.
 */
const leg = defineModel<PresetOutputInput>({ required: true })
const nodes = useNodesStore()

const a = computed(() => leg.value.advanced)
const summary = computed(() => advancedSummary(leg.value))
const problems = computed(() => advancedProblems(leg.value))
const open = ref(summary.value.length > 0 || Object.keys(problems.value).length > 0)

function set<K extends keyof OutputAdvanced>(key: K, value: OutputAdvanced[K]) {
  leg.value = { ...leg.value, advanced: { ...leg.value.advanced, [key]: value } }
}

function reset() {
  leg.value = { ...leg.value, advanced: defaultAdvanced() }
  channelText.value = ''
}

/** Settings that differ from their default, marked with a dot. */
const changed = computed(() => {
  const d = defaultAdvanced()
  return (key: keyof OutputAdvanced) => JSON.stringify(a.value[key]) !== JSON.stringify(d[key])
})

const bitrated = computed(() => hasBitrate(leg.value.codec))
const prores = computed(() => isProRes(leg.value))

// ── Video ─────────────────────────────────────────────────────────────────────

const encoderOptions = computed(() =>
  (['auto', 'hardware', 'software'] as const).map((value) => {
    const reason = value === 'auto' ? null : encoderChoiceProblem(leg.value, value)
    return {
      value,
      label: { auto: 'Auto', hardware: 'Hardware', software: 'Software' }[value],
      disabled: !!reason,
      reason: reason ?? undefined,
    }
  }),
)

/** What the encoder resolves to on each node, for the help popover. */
const encoderNote = computed(() => {
  const on = encoderOnNodes(leg.value, nodes.nodes)
  return on.length ? on.map((n) => `${n.node}: ${n.encoder}`).join(' · ') : undefined
})

const quality = computed({
  get: () => [a.value.quality ?? DEFAULT_QUALITY],
  set: ([v]: number[]) => set('quality', v === DEFAULT_QUALITY ? null : (v ?? null)),
})

/** Optional numbers: blank is the default (null). */
const numberField = (key: 'keyframe_secs' | 'audio_bitrate_kbps') =>
  computed({
    get: () => a.value[key] ?? '',
    set: (v: string | number) => set(key, v === '' || v == null ? null : Number(v)),
  })
const keyframes = numberField('keyframe_secs')
const audioBitrate = numberField('audio_bitrate_kbps')

// ── Audio ─────────────────────────────────────────────────────────────────────

const CHANNEL_OPTIONS: { value: AudioChannels; label: string }[] = [
  { value: 'all', label: 'All' },
  { value: 'stereo', label: 'Stereo mix' },
  { value: 'pick', label: 'Pick…' },
]

/** The typed channel list, kept as typed; the parsed list is what's stored. */
const channelText = ref(formatChannels(a.value.channel_pick))
const channelTextBad = computed(() => channelText.value.trim() !== '' && !parseChannels(channelText.value))
watch(channelText, (text) => set('channel_pick', parseChannels(text) ?? []))

const audio = computed(() => resolvedAudio(leg.value))
</script>

<template>
  <Collapsible v-model:open="open" class="col-span-2 lg:col-span-4 rounded-md border border-border/70 bg-muted/20">
    <div class="flex items-center gap-2 pr-1.5 h-8">
      <CollapsibleTrigger class="flex items-center gap-1.5 flex-1 min-w-0 h-full pl-2 text-left">
        <ChevronRight class="size-3.5 shrink-0 text-muted-foreground transition-transform" :class="open && 'rotate-90'" />
        <span class="text-xs font-medium">Advanced</span>
        <span v-if="summary.length" class="text-xs text-muted-foreground truncate">· {{ summary.join(' · ') }}</span>
        <span v-else class="text-xs text-muted-foreground/70">· defaults</span>
        <span v-if="Object.keys(problems).length" class="text-xs text-destructive shrink-0">needs attention</span>
      </CollapsibleTrigger>
      <button
        v-if="summary.length"
        type="button"
        class="h-6 px-1.5 inline-flex items-center gap-1 rounded text-[11px] text-muted-foreground hover:text-foreground hover:bg-accent"
        @click="reset"
      >
        <RotateCcw class="size-3" /> Reset
      </button>
    </div>

    <CollapsibleContent>
      <div class="px-3 pb-3 pt-1 space-y-3 border-t border-border/70">
        <!-- Video -->
        <div class="grid grid-cols-2 lg:grid-cols-4 gap-3 [&>*]:min-w-0">
          <h4 class="group-title">Video</h4>

          <FormField v-if="leg.codec !== 'uncompressed'" :help="FIELD_HELP.encoder" :help-note="encoderNote">
            <template #label>Encoder <span v-if="changed('encoder')" class="dot" /></template>
            <OptionSelect
              :model-value="a.encoder"
              :options="encoderOptions"
              @update:model-value="(v: EncoderChoice) => set('encoder', v)"
            />
            <span v-if="problems.encoder" class="text-[11px] text-destructive">{{ problems.encoder }}</span>
          </FormField>
          <FormField v-else label="Encoder" :help="FIELD_HELP.encoderFixed">
            <span class="fixed-value">None</span>
          </FormField>

          <FormField v-if="bitrated" :help="FIELD_HELP.rateControl">
            <template #label>Rate control <span v-if="changed('rate_control')" class="dot" /></template>
            <OptionSelect
              :model-value="a.rate_control"
              :options="RATE_CONTROL_OPTIONS"
              @update:model-value="(v: RateControl) => set('rate_control', v)"
            />
          </FormField>
          <FormField v-else label="Rate control" :help="FIELD_HELP.rateControlFixed">
            <span class="fixed-value">Set by the codec</span>
          </FormField>

          <FormField v-if="bitrated && a.rate_control === 'quality'" :help="FIELD_HELP.quality">
            <template #label>Quality <span v-if="changed('quality')" class="dot" /></template>
            <div class="h-7 flex items-center gap-2.5">
              <Slider v-model="quality" :min="1" :max="100" :step="1" class="flex-1" aria-label="Quality" />
              <span class="num text-xs w-6 text-right">{{ quality[0] }}</span>
            </div>
          </FormField>

          <FormField v-if="bitrated && usesSpeedPreset(leg)" :help="FIELD_HELP.speedPreset">
            <template #label>Speed preset <span v-if="changed('speed_preset')" class="dot" /></template>
            <OptionSelect
              :model-value="a.speed_preset ?? DEFAULT_SPEED"
              :options="SPEED_OPTIONS"
              @update:model-value="(v: SpeedPreset) => set('speed_preset', v === DEFAULT_SPEED ? null : v)"
            />
          </FormField>
          <FormField v-else label="Speed preset" :help="FIELD_HELP.speedPresetFixed">
            <span class="fixed-value">{{ a.encoder === 'hardware' ? 'Set by the hardware' : 'Set by the encoder' }}</span>
          </FormField>

          <FormField v-if="bitrated" :help="FIELD_HELP.keyframes">
            <template #label>Keyframe interval (s) <span v-if="changed('keyframe_secs')" class="dot" /></template>
            <Input
              v-model="keyframes"
              type="number"
              min="0.1"
              max="60"
              step="0.5"
              :placeholder="String(DEFAULT_KEYFRAME_SECS)"
              class="num"
              :aria-invalid="!!problems.keyframe_secs"
            />
            <span v-if="problems.keyframe_secs" class="text-[11px] text-destructive">{{ problems.keyframe_secs }}</span>
          </FormField>
          <FormField v-else label="Keyframe interval" :help="FIELD_HELP.keyframesFixed">
            <span class="fixed-value">Every frame</span>
          </FormField>

          <FormField v-if="!prores" :help="FIELD_HELP.deinterlace">
            <template #label>Deinterlace <span v-if="changed('deinterlace')" class="dot" /></template>
            <OptionSelect
              :model-value="a.deinterlace"
              :options="[
                { value: 'auto', label: 'Auto' },
                { value: 'off', label: 'Off' },
              ]"
              @update:model-value="(v: Deinterlace) => set('deinterlace', v)"
            />
          </FormField>
          <FormField v-else label="Deinterlace" :help="FIELD_HELP.deinterlaceFixed">
            <span class="fixed-value">Kept as recorded</span>
          </FormField>
        </div>

        <!-- Audio -->
        <div class="grid grid-cols-2 lg:grid-cols-4 gap-3 [&>*]:min-w-0">
          <h4 class="group-title">Audio</h4>

          <FormField :help="FIELD_HELP.audioCodec">
            <template #label>Audio codec <span v-if="changed('audio_codec')" class="dot" /></template>
            <OptionSelect
              :model-value="a.audio_codec"
              :options="audioCodecOptions(leg)"
              @update:model-value="(v: AudioCodecChoice) => set('audio_codec', v)"
            />
            <span v-if="problems.audio_codec" class="text-[11px] text-destructive">{{ problems.audio_codec }}</span>
          </FormField>

          <FormField v-if="audio !== 'pcm'" :help="FIELD_HELP.audioBitrate">
            <template #label>Audio bitrate (kbps) <span v-if="changed('audio_bitrate_kbps')" class="dot" /></template>
            <Input
              v-model="audioBitrate"
              type="number"
              min="32"
              max="512"
              :placeholder="String(AUDIO_DEFAULT_KBPS[audio])"
              class="num"
              :aria-invalid="!!problems.audio_bitrate_kbps"
            />
            <span v-if="problems.audio_bitrate_kbps" class="text-[11px] text-destructive">{{ problems.audio_bitrate_kbps }}</span>
          </FormField>
          <FormField v-else label="Audio bitrate" :help="FIELD_HELP.audioBitrateFixed">
            <span class="fixed-value">Uncompressed</span>
          </FormField>

          <FormField :help="FIELD_HELP.channels" :class="a.audio_channels === 'pick' && 'col-span-2'">
            <template #label>Channels <span v-if="changed('audio_channels')" class="dot" /></template>
            <div class="flex gap-1.5">
              <OptionSelect
                :model-value="a.audio_channels"
                :options="CHANNEL_OPTIONS"
                :class="a.audio_channels === 'pick' ? 'w-28 shrink-0' : ''"
                @update:model-value="(v: AudioChannels) => set('audio_channels', v)"
              />
              <Input
                v-if="a.audio_channels === 'pick'"
                v-model="channelText"
                placeholder="e.g. 1-2 or 3, 4"
                aria-label="Channels to record"
                class="num h-7 flex-1 min-w-0"
                :aria-invalid="channelTextBad || !!problems.channel_pick"
              />
            </div>
            <span v-if="channelTextBad" class="text-[11px] text-destructive">Use numbers 1–64 and ranges, e.g. 1-2, 5</span>
            <span v-else-if="problems.channel_pick" class="text-[11px] text-destructive">{{ problems.channel_pick }}</span>
          </FormField>
        </div>
      </div>
    </CollapsibleContent>
  </Collapsible>
</template>

<style scoped>
@reference "@/style.css";
.group-title {
  @apply col-span-2 lg:col-span-4 -mb-1 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground/80;
}
.fixed-value {
  @apply h-7 flex items-center text-xs text-muted-foreground;
}
.dot {
  @apply inline-block size-1.5 rounded-full bg-primary align-middle;
}
</style>
