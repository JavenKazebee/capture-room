<script setup lang="ts">
import { computed } from 'vue'
import type { VideoFormat } from '@/types/generated/VideoFormat'
import { findFramerate, findResolution, FRAMERATE_OPTIONS, RESOLUTION_OPTIONS } from '@/lib/videoPresets'
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'

/** By default the source's first format is held (a later change is scaled); a fixed one never changes size. */
const format = defineModel<VideoFormat | null>({ required: true })

const AUTO = 'auto'
const size = computed({
  get: () => (format.value ? `${format.value.width}x${format.value.height}` : AUTO),
  set: (v: string) => {
    const r = findResolution(v)
    if (!r) {
      format.value = null
      return
    }
    format.value = { fps_num: 30, fps_den: 1, ...format.value, width: r.w, height: r.h }
  },
})

const rate = computed({
  get: () => (format.value ? `${format.value.fps_num}/${format.value.fps_den}` : '30/1'),
  set: (v: string) => {
    const r = findFramerate(v)
    if (r && format.value) format.value = { ...format.value, fps_num: r.n, fps_den: r.d }
  },
})

const sizeOptions = [{ value: AUTO, label: "Source's first format" }, ...RESOLUTION_OPTIONS]
</script>

<template>
  <div class="grid grid-cols-2 gap-3">
    <FormField label="Output size">
      <OptionSelect v-model="size" :options="sizeOptions" />
    </FormField>
    <FormField label="Output rate">
      <OptionSelect v-model="rate" :options="FRAMERATE_OPTIONS" :disabled="!format" />
    </FormField>
  </div>
</template>
