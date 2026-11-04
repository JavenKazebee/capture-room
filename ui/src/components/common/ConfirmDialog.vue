<script setup lang="ts">
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button, type ButtonVariants } from '@/components/ui/button'

/** Confirmation for a destructive or risky action. Open it with `v-model:open`. */
withDefaults(
  defineProps<{
    title: string
    description?: string
    confirmLabel?: string
    confirmVariant?: ButtonVariants['variant']
  }>(),
  { confirmLabel: 'Delete', confirmVariant: 'destructive' },
)
const open = defineModel<boolean>('open', { required: true })
const emit = defineEmits<{ confirm: [] }>()

// Emit before closing: reka's AlertDialogAction closes first, and callers that
// clear their state on close (e.g. "Stop all") would then confirm against nothing.
function confirm() {
  emit('confirm')
  open.value = false
}
</script>

<template>
  <AlertDialog v-model:open="open">
    <AlertDialogContent>
      <AlertDialogHeader>
        <AlertDialogTitle>{{ title }}</AlertDialogTitle>
        <AlertDialogDescription v-if="description || $slots.default">
          <slot>{{ description }}</slot>
        </AlertDialogDescription>
      </AlertDialogHeader>
      <AlertDialogFooter>
        <AlertDialogCancel>Cancel</AlertDialogCancel>
        <Button :variant="confirmVariant" @click="confirm">{{ confirmLabel }}</Button>
      </AlertDialogFooter>
    </AlertDialogContent>
  </AlertDialog>
</template>
