<script setup lang="ts">
import { ref } from 'vue'
import { RotateCw } from '@lucide/vue'
import { cn } from '@/lib/utils'
import { Button } from '@/components/ui/button'

/** A failed load, shown next to whatever data is still there, with a way to try again. */
const props = defineProps<{
  error: string | null | undefined
  retry: () => unknown
  class?: string
}>()

const retrying = ref(false)

async function onRetry() {
  retrying.value = true
  try {
    await props.retry()
  } finally {
    retrying.value = false
  }
}
</script>

<template>
  <div v-if="error" role="alert" :class="cn('flex items-center gap-2 text-xs text-destructive', props.class)">
    <span class="min-w-0 break-words">{{ error }}</span>
    <Button size="xs" variant="destructive" class="shrink-0" :disabled="retrying" @click="onRetry">
      <RotateCw :class="{ 'animate-spin': retrying }" /> Retry
    </Button>
  </div>
</template>
