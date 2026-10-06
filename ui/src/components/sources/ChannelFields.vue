<script setup lang="ts">
import { computed } from 'vue'
import { Plus, X } from '@lucide/vue'
import type { ChannelConfig } from '@/types/generated/ChannelConfig'
import { Input } from '@/components/ui/input'
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'
import { FRAMERATES, FRAMERATE_OPTIONS, RESOLUTIONS, RESOLUTION_OPTIONS } from '@/lib/videoPresets'

/** A playout channel: its fixed program format, and where it's sent. */
const props = defineProps<{
  /** The channel's name: what an NDI output is called unless renamed. */
  name: string
}>()

const cfg = defineModel<ChannelConfig>({ required: true })

const CHANNEL_OPTIONS = [
  { value: 1, label: 'Mono' },
  { value: 2, label: 'Stereo' },
  { value: 6, label: '5.1' },
  { value: 8, label: '7.1' },
]

const resolution = computed({
  get: () => `${cfg.value.format.width}x${cfg.value.format.height}`,
  set: (v: string) => {
    const r = RESOLUTIONS.find((r) => `${r.w}x${r.h}` === v)
    if (r) cfg.value = { ...cfg.value, format: { ...cfg.value.format, width: r.w, height: r.h } }
  },
})

const framerate = computed({
  get: () => `${cfg.value.format.fps_num}/${cfg.value.format.fps_den}`,
  set: (v: string) => {
    const r = FRAMERATES.find((r) => `${r.n}/${r.d}` === v)
    if (r) cfg.value = { ...cfg.value, format: { ...cfg.value.format, fps_num: r.n, fps_den: r.d } }
  },
})

function setNdiName(i: number, ndi_name: string) {
  const outputs = cfg.value.outputs.map((o, j) => (j === i ? { ...o, ndi_name: ndi_name || null } : o))
  cfg.value = { ...cfg.value, outputs }
}

function addNdi() {
  cfg.value = { ...cfg.value, outputs: [...cfg.value.outputs, { type: 'ndi', ndi_name: null }] }
}

function remove(i: number) {
  cfg.value = { ...cfg.value, outputs: cfg.value.outputs.filter((_, j) => j !== i) }
}
</script>

<template>
  <div class="col-span-2 grid grid-cols-2 gap-3">
    <FormField label="Resolution">
      <OptionSelect v-model="resolution" :options="RESOLUTION_OPTIONS" />
    </FormField>
    <FormField label="Frame rate">
      <OptionSelect v-model="framerate" :options="FRAMERATE_OPTIONS" />
    </FormField>
    <FormField label="Audio">
      <OptionSelect v-model="cfg.audio_channels" :options="CHANNEL_OPTIONS" />
    </FormField>
    <p class="col-span-2 -mt-1 text-xs text-muted-foreground">
      Every clip is scaled and converted to this format, so outputs never change format between clips.
    </p>

    <div class="col-span-2 flex flex-col gap-1.5">
      <span class="text-xs text-muted-foreground">Outputs</span>
      <div
        v-for="(o, i) in cfg.outputs"
        :key="i"
        class="flex items-center gap-2 rounded-md border border-border px-2.5 py-1.5"
      >
        <span class="text-[10px] font-semibold uppercase tracking-wide text-muted-foreground w-8">NDI</span>
        <Input
          :model-value="o.ndi_name ?? ''"
          class="h-7 flex-1"
          :placeholder="props.name.trim() || 'Channel name'"
          :aria-label="`NDI name for output ${i + 1}`"
          @update:model-value="(v) => setNdiName(i, String(v))"
        />
        <button
          type="button"
          class="size-7 grid place-items-center rounded text-muted-foreground hover:text-foreground hover:bg-accent"
          :aria-label="`Remove output ${i + 1}`"
          @click="remove(i)"
        >
          <X class="size-3.5" />
        </button>
      </div>
      <button
        type="button"
        class="flex items-center gap-1 text-xs text-primary hover:underline w-fit"
        @click="addNdi"
      >
        <Plus class="size-3.5" /> Add NDI output
      </button>
      <p class="text-xs text-muted-foreground">
        NDI receivers see it as <span class="font-mono">NODE (name)</span>. The program can also be recorded from Record like any source.
      </p>
    </div>
  </div>
</template>
