<script setup lang="ts">
import { computed } from 'vue'
import type { VideoFormat } from '@/types/generated/VideoFormat'
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'
import { AUDIO_CHANNEL_OPTIONS, findFramerate, findResolution, FRAMERATE_OPTIONS, RESOLUTION_OPTIONS } from '@/lib/videoFormats'

/** A channel's fixed program format. */
const format = defineModel<VideoFormat>('format', { required: true })
const audioChannels = defineModel<number>('audioChannels', { required: true })

const resolution = computed({
  get: () => `${format.value.width}x${format.value.height}`,
  set: (v: string) => {
    const r = findResolution(v)
    if (r) format.value = { ...format.value, width: r.w, height: r.h }
  },
})

const framerate = computed({
  get: () => `${format.value.fps_num}/${format.value.fps_den}`,
  set: (v: string) => {
    const r = findFramerate(v)
    if (r) format.value = { ...format.value, fps_num: r.n, fps_den: r.d }
  },
})
</script>

<template>
  <FormField label="Resolution">
    <OptionSelect v-model="resolution" :options="RESOLUTION_OPTIONS" />
  </FormField>
  <FormField label="Frame rate">
    <OptionSelect v-model="framerate" :options="FRAMERATE_OPTIONS" />
  </FormField>
  <FormField label="Audio">
    <OptionSelect v-model="audioChannels" :options="AUDIO_CHANNEL_OPTIONS" />
  </FormField>
  <p class="col-span-2 -mt-1 text-xs text-muted-foreground">
    Every clip is scaled and converted to this format, so outputs never change format between clips.
  </p>
</template>
