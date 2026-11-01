<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { nodeApi } from '@/composables/useApi'
import type { NodeDto } from '@/types/generated/NodeDto'
import type { UpdateStatusDto } from '@/types/generated/UpdateStatusDto'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import ReleaseNotes from './ReleaseNotes.vue'

/**
 * A node's version, opening that version's release highlights when the node has them. The node
 * only has notes for the newest release, so a node with an update waiting shows a plain version.
 */
const props = defineProps<{ node: NodeDto }>()

const status = ref<UpdateStatusDto | null>(null)

// The node checks for updates as it starts, so right after a restart its notes may not be in yet.
const RETRY_MS = 5_000
const RETRIES = 6

async function look(version: string, tries = RETRIES) {
  try {
    const s = await nodeApi(props.node.id)<UpdateStatusDto>('/update')
    if (version !== props.node.version) return
    status.value = s
    if (s.phase !== 'checking') return
  } catch {
    // Retried below.
  }
  if (tries > 0 && version === props.node.version) setTimeout(() => look(version, tries - 1), RETRY_MS)
}

watch(
  () => [props.node.version, props.node.healthy] as const,
  ([version, healthy]) => {
    status.value = null
    if (healthy && version) look(version)
  },
  { immediate: true },
)

const notes = computed(() => {
  const s = status.value
  return s?.notes && s.latest === props.node.version ? s : null
})
</script>

<template>
  <Popover v-if="notes">
    <PopoverTrigger
      class="num shrink-0 underline decoration-dotted underline-offset-2 hover:text-foreground"
      title="What's new in this version"
    >
      v{{ node.version }}
    </PopoverTrigger>
    <PopoverContent align="start" class="w-96 max-h-[60vh] overflow-y-auto p-3">
      <p class="text-sm font-medium">What's new in <span class="num">{{ node.version }}</span></p>
      <ReleaseNotes :notes="notes.notes!" :url="notes.notes_url" />
    </PopoverContent>
  </Popover>
  <span v-else class="num shrink-0">v{{ node.version }}</span>
</template>
