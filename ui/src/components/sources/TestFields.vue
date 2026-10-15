<script setup lang="ts">
import { computed } from 'vue'
import type { AudioTestSignal } from '@/types/generated/AudioTestSignal'
import type { TestSourceConfig } from '@/types/generated/TestSourceConfig'
import type { VideoTestPattern } from '@/types/generated/VideoTestPattern'
import { Input } from '@/components/ui/input'
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'
import { FRAMERATES, FRAMERATE_OPTIONS, RESOLUTIONS, RESOLUTION_OPTIONS } from '@/lib/videoPresets'

/** A synthetic feed: a video pattern plus a test audio signal. */
const cfg = defineModel<TestSourceConfig>({ required: true })

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

const CHANNEL_OPTIONS = [
  { value: 1, label: 'Mono' },
  { value: 2, label: 'Stereo' },
  { value: 6, label: '5.1' },
  { value: 8, label: '7.1' },
]

const resolutionKey = computed({
  get: () => `${cfg.value.width}x${cfg.value.height}`,
  set: (v: string) => {
    const found = RESOLUTIONS.find((r) => `${r.w}x${r.h}` === v)
    if (found) { cfg.value.width = found.w; cfg.value.height = found.h }
  },
})

const framerateKey = computed({
  get: () => `${cfg.value.fps_num}/${cfg.value.fps_den}`,
  set: (v: string) => {
    const found = FRAMERATES.find((r) => `${r.n}/${r.d}` === v)
    if (found) { cfg.value.fps_num = found.n; cfg.value.fps_den = found.d }
  },
})
</script>

<template>
  <div class="col-span-2 grid grid-cols-2 gap-3">
    <FormField label="Video pattern" class="col-span-2">
      <OptionSelect v-model="cfg.pattern" :options="VIDEO_PATTERNS" />
    </FormField>

    <FormField label="Resolution">
      <OptionSelect v-model="resolutionKey" :options="RESOLUTION_OPTIONS" />
    </FormField>

    <FormField label="Frame rate">
      <OptionSelect v-model="framerateKey" :options="FRAMERATE_OPTIONS" />
    </FormField>

    <FormField label="Audio signal">
      <OptionSelect v-model="cfg.audio_signal" :options="AUDIO_SIGNALS" />
    </FormField>

    <FormField>
      <template #label>
        Frequency (Hz)
        <span v-if="cfg.audio_signal !== 'tone'" class="opacity-40">— n/a</span>
      </template>
      <Input
        v-model.number="cfg.frequency"
        type="number"
        :disabled="cfg.audio_signal !== 'tone'"
        placeholder="440"
        min="20"
        max="20000"
      />
    </FormField>

    <FormField label="Audio channels">
      <OptionSelect v-model="cfg.channels" :options="CHANNEL_OPTIONS" />
    </FormField>
  </div>
</template>
