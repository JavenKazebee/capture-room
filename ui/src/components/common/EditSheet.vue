<script setup lang="ts">
import { Loader2 } from '@lucide/vue'
import { Button } from '@/components/ui/button'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'

/**
 * A create/edit form in a right-side sheet, keeping the list visible behind
 * it: title, form body, error line, Cancel/Save. Mount it with `v-if`.
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
  <Sheet :open="true" @update:open="(v) => !v && emit('close')">
    <SheetContent
      class="flex flex-col gap-0 p-0 w-full"
      :class="wide ? 'sm:max-w-2xl' : 'sm:max-w-md'"
    >
      <SheetHeader class="border-b border-border">
        <SheetTitle>{{ title }}</SheetTitle>
        <SheetDescription :class="!description && 'sr-only'">{{ description ?? title }}</SheetDescription>
      </SheetHeader>

      <form class="flex-1 overflow-y-auto p-4" @submit.prevent="emit('save')">
        <slot />
        <button type="submit" hidden />
      </form>

      <SheetFooter class="border-t border-border flex-row items-center justify-end gap-2">
        <p v-if="error" class="mr-auto text-xs text-destructive">{{ error }}</p>
        <Button variant="outline" :disabled="saving" @click="emit('close')">Cancel</Button>
        <Button v-if="!noSave" :disabled="saving" class="gap-1.5" @click="emit('save')">
          <Loader2 v-if="saving" class="size-3.5 animate-spin" />
          Save
        </Button>
      </SheetFooter>
    </SheetContent>
  </Sheet>
</template>
