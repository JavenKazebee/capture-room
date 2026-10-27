<script setup lang="ts">
import { Input } from '@/components/ui/input'
import FormField from '@/components/FormField.vue'

const bitrate = defineModel<number>({ required: true })
</script>

<template>
  <FormField label="Bitrate (kbps)">
    <Input
      :model-value="bitrate"
      type="number"
      min="100"
      max="100000"
      class="h-7"
      @update:model-value="(v) => (bitrate = Number(v))"
    />
  </FormField>
</template>
