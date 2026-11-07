<script setup lang="ts">
import { computed } from 'vue'
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'
import { FIELD_HELP } from '@/lib/fieldHelp'
import { ltcChannelOptions } from '@/lib/ltc'

/** Which of a live source's audio channels carries LTC timecode, if any. */
const props = defineProps<{
  /** The feed's audio channels; `null` when it's silent. */
  channels: number | null
}>()

const channel = defineModel<number | null>({ required: true })

const options = computed(() => ltcChannelOptions(props.channels, channel.value))
/** 0 stands for off: the select takes only numbers. */
const picked = computed({
  get: () => channel.value ?? 0,
  set: (v: number) => (channel.value = v || null),
})
const hint = computed(() => {
  if (props.channels === null) return 'Give the feed audio to read LTC from it.'
  return null
})
</script>

<template>
  <FormField label="LTC timecode" :help="FIELD_HELP.ltc" class="col-span-2">
    <OptionSelect v-model="picked" :options="options" :disabled="channels === null && !channel" />
  </FormField>
  <p v-if="hint" class="col-span-2 -mt-1.5 text-xs text-muted-foreground">{{ hint }}</p>
</template>
