<script setup lang="ts">
import { computed } from 'vue'
import { useRouter } from 'vue-router'
import { useNow } from '@vueuse/core'
import { HardDrive, ScrollText } from '@lucide/vue'
import { wsStatus } from '@/composables/useWebSocket'
import { formatBytes } from '@/lib/format'
import { useEventsStore } from '@/stores/events'
import { useNodesStore } from '@/stores/nodes'
import { useRecordingsStore } from '@/stores/recordings'
import { useStorageStore } from '@/stores/storage'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import StatusDot from '@/components/common/StatusDot.vue'

defineProps<{ logOpen: boolean }>()
defineEmits<{ toggleLog: [] }>()

const router = useRouter()
const nodes = useNodesStore()
const recordings = useRecordingsStore()
const storage = useStorageStore()
const events = useEventsStore()

const now = useNow({ interval: 1000 })
const clock = computed(() => now.value.toLocaleTimeString([], { hour12: false }))

const healthy = computed(() => nodes.nodes.filter((n) => n.healthy).length)
const live = computed(() => recordings.activeSessions.length)

const lowest = computed(() => {
  const l = storage.lowest
  if (!l) return null
  const { volume } = l
  const pct = volume.total_bytes ? volume.available_bytes / volume.total_bytes : 1
  return { ...l, pct, status: pct < 0.1 ? 'error' : pct < 0.25 ? 'warn' : 'ok' } as const
})

const connection = computed(() => {
  switch (wsStatus.value) {
    case 'connected':
      return { status: 'ok', label: 'Connected' } as const
    case 'connecting':
      return { status: 'warn', label: 'Connecting…' } as const
    default:
      return { status: 'error', label: 'Disconnected — reconnecting' } as const
  }
})
</script>

<template>
  <footer
    class="h-6 shrink-0 flex items-stretch text-[11px] border-t border-sidebar-border bg-sidebar text-muted-foreground select-none"
  >
    <!-- Connection -->
    <Tooltip>
      <TooltipTrigger as-child>
        <span
          class="status-item"
          :class="wsStatus !== 'connected' && 'bg-warning/15 text-warning font-medium'"
        >
          <StatusDot :status="connection.status" />
          {{ connection.label }}
        </span>
      </TooltipTrigger>
      <TooltipContent side="top">Live event stream from the server</TooltipContent>
    </Tooltip>

    <!-- Nodes -->
    <button class="status-item" @click="router.push('/nodes')">
      <StatusDot :status="healthy === nodes.nodes.length ? 'ok' : 'warn'" />
      <span class="num">{{ healthy }}/{{ nodes.nodes.length }}</span> nodes
      <span v-if="nodes.isController" class="text-info">· controller</span>
    </button>

    <!-- Recording -->
    <button
      class="status-item"
      :class="live ? 'bg-tally text-tally-foreground font-semibold' : ''"
      @click="router.push('/multiview')"
    >
      <template v-if="live">
        <span class="size-1.5 rounded-full bg-current animate-tally" />
        <span class="num">{{ live }}</span> recording
      </template>
      <template v-else><StatusDot status="off" /> Idle</template>
    </button>

    <!-- Lowest storage -->
    <Tooltip v-if="lowest">
      <TooltipTrigger as-child>
        <button
          class="status-item"
          :class="{
            'text-warning': lowest.status === 'warn',
            'text-destructive font-medium': lowest.status === 'error',
          }"
          @click="router.push('/nodes')"
        >
          <HardDrive class="size-3" />
          <span class="num">{{ formatBytes(lowest.volume.available_bytes) }}</span> free
        </button>
      </TooltipTrigger>
      <TooltipContent side="top">
        Lowest free space: <span class="num">{{ lowest.volume.mount_point }}</span>
        on {{ nodes.nameOf(lowest.nodeId) }} ({{ Math.round(lowest.pct * 100) }}% free)
      </TooltipContent>
    </Tooltip>

    <div class="flex-1" />

    <button class="status-item" :class="logOpen && 'text-foreground'" @click="$emit('toggleLog')">
      <ScrollText class="size-3" />
      Log
      <span
        v-if="events.unseenErrors"
        class="num rounded-sm bg-destructive/20 text-destructive px-1 font-medium"
      >{{ events.unseenErrors }}</span>
      <span class="opacity-60">Ctrl J</span>
    </button>
    <span class="status-item num">{{ clock }}</span>
  </footer>
</template>

<style scoped>
@reference "@/style.css";
.status-item {
  @apply flex items-center gap-1.5 px-2.5 whitespace-nowrap;
}
button.status-item {
  @apply hover:bg-sidebar-accent hover:text-foreground;
}
</style>
