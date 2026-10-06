<script setup lang="ts">
import { computed } from 'vue'
import type { LiveVideoFormat } from '@/types/generated/LiveVideoFormat'
import { FRAMERATES, FRAMERATE_OPTIONS, RESOLUTIONS, RESOLUTION_OPTIONS } from '@/lib/videoPresets'
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'

/**
 * A live source's output format: by default it takes the source's first
 * format and holds it (a source that comes back in another one is scaled);
 * a fixed format is set from the start, so the feed never changes size.
 */
const format = defineModel<LiveVideoFormat | null>({ required: true })

const AUTO = 'auto'
const size = computed({
  get: () => (format.value ? `${format.value.width}x${format.value.height}` : AUTO),
  set: (v: string) => {
    const r = RESOLUTIONS.find((r) => `${r.w}x${r.h}` === v)
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
    const r = FRAMERATES.find((r) => `${r.n}/${r.d}` === v)
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
