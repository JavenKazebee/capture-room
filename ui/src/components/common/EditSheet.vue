<script setup lang="ts">
import { Loader2 } from '@lucide/vue'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'

/**
 * A create/edit form in a centered dialog. Mount with `v-if`; `#footer-start` holds e.g. Delete.
 * Dialogs opened from inside it (a file browser, a confirm) stack above it and take Esc first.
 */
defineProps<{
  title: string
  description?: string
  error?: string | null
  saving?: boolean
  wide?: boolean
  /** Hide Save, for a step that has nothing to save yet. */
  noSave?: boolean
}>()

const emit = defineEmits<{ close: []; save: [] }>()
</script>

<template>
  <Dialog :open="true" @update:open="(v) => !v && emit('close')">
    <DialogContent
      class="flex flex-col gap-0 p-0 max-h-[85vh] overflow-hidden"
      :class="wide ? 'sm:max-w-2xl' : 'sm:max-w-lg'"
    >
      <DialogHeader class="shrink-0 border-b border-border px-4 py-3 pr-10">
        <DialogTitle>{{ title }}</DialogTitle>
        <DialogDescription :class="!description && 'sr-only'">{{ description ?? title }}</DialogDescription>
      </DialogHeader>

      <form class="flex-1 min-h-0 overflow-y-auto p-4" @submit.prevent="emit('save')">
        <slot />
        <button type="submit" hidden />
      </form>

      <DialogFooter class="shrink-0 border-t border-border px-4 py-3 flex-row items-center justify-end gap-2">
        <slot name="footer-start" />
        <p v-if="error" class="mr-auto text-xs text-destructive">{{ error }}</p>
        <div v-else-if="$slots['footer-start']" class="mr-auto" />
        <Button variant="outline" :disabled="saving" @click="emit('close')">Cancel</Button>
        <Button v-if="!noSave" :disabled="saving" class="gap-1.5" @click="emit('save')">
          <Loader2 v-if="saving" class="size-3.5 animate-spin" />
          Save
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
