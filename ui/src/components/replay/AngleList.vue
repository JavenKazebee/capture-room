<script setup lang="ts">
import { useNodesStore } from '@/stores/nodes'
import type { Source } from '@/stores/sources'
import StatusDot from '@/components/common/StatusDot.vue'
import { held } from './held'
import type { Angle } from './useReplayControl'

/**
 * Every angle with a buffer. Those on the channel's node are numbered (the
 * 1–9 keys) and can be picked; the others play on their own node's channels.
 */
const props = defineProps<{
  angles: Angle[]
  usableAngles: Angle[]
  selectedKey: string | null
  /** The angle on air, if a replay is showing. */
  showingKey: string | null
}>()
const emit = defineEmits<{ select: [source: Source] }>()

const nodes = useNodesStore()
const number = (a: Angle) => props.usableAngles.indexOf(a) + 1
</script>

<template>
  <nav class="w-60 shrink-0 border-r border-border overflow-y-auto p-2 flex flex-col gap-2" aria-label="Angles">
    <button
      v-for="a in angles"
      :key="a.source.key"
      class="text-left rounded-md border p-1.5 flex flex-col gap-1"
      :class="[
        a.source.key === selectedKey ? 'border-primary ring-1 ring-primary' : 'border-border hover:bg-accent/40',
        !number(a) && 'opacity-50 cursor-not-allowed',
        showingKey === a.source.key && 'bg-success/10',
      ]"
      :disabled="!number(a)"
      :title="number(a) ? undefined : `On ${nodes.nameOf(a.buffer.node_id)}: it plays on that node's channels`"
      :aria-current="a.source.key === selectedKey || undefined"
      @click="emit('select', a.source)"
    >
      <span class="flex items-center gap-1.5 text-xs min-w-0">
        <span class="num text-[10px] rounded bg-muted px-1 text-muted-foreground shrink-0">
          {{ number(a) || '–' }}
        </span>
        <StatusDot :status="a.buffer.error ? 'error' : a.buffer.encoder ? 'ok' : 'off'" />
        <span class="font-medium truncate">{{ a.source.display_name }}</span>
        <span class="flex-1" />
        <span class="num text-[10px] text-muted-foreground shrink-0">{{ held(a.buffer) }}</span>
      </span>
      <span v-if="a.buffer.error" class="text-[11px] text-destructive line-clamp-2">{{ a.buffer.error }}</span>
    </button>
  </nav>
</template>
