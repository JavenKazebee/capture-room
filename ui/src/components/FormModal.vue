<script setup lang="ts">
import { Button } from '@/components/ui/button'

/** A create/edit dialog: title, form body, error line, Cancel/Save. */
defineProps<{
  title: string
  error?: string | null
  saving?: boolean
  wide?: boolean
}>()

defineEmits<{ close: []; save: [] }>()
</script>

<template>
  <div
    class="fixed inset-0 bg-black/40 flex items-center justify-center p-4 z-50"
    @click.self="$emit('close')"
  >
    <div
      class="bg-card border border-border rounded-lg w-full max-h-[90vh] overflow-y-auto p-5"
      :class="wide ? 'max-w-2xl' : 'max-w-lg'"
    >
      <h2 class="text-lg font-semibold mb-4">{{ title }}</h2>

      <slot />

      <p v-if="error" class="text-xs text-destructive mt-3">{{ error }}</p>

      <div class="flex justify-end gap-2 mt-5">
        <Button variant="outline" :disabled="saving" @click="$emit('close')">Cancel</Button>
        <Button :disabled="saving" @click="$emit('save')">
          {{ saving ? 'Saving…' : 'Save' }}
        </Button>
      </div>
    </div>
  </div>
</template>
