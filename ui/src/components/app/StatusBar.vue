<script setup lang="ts">
import { computed } from 'vue'
import { useRouter } from 'vue-router'
import { useNow } from '@vueuse/core'
import { HardDrive, Search } from '@lucide/vue'
import { wsStatus } from '@/composables/useWebSocket'
import { formatBytes } from '@/lib/format'
import { useNodesStore } from '@/stores/nodes'
import { useRecordingsStore } from '@/stores/recordings'
import { useStorageStore } from '@/stores/storage'
import { SidebarTrigger } from '@/components/ui/sidebar'
import { Separator } from '@/components/ui/separator'
import { Kbd } from '@/components/ui/kbd'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import StatusDot from '@/components/common/StatusDot.vue'

defineEmits<{ command: [] }>()

const router = useRouter()
const nodes = useNodesStore()
const recordings = useRecordingsStore()
const storage = useStorageStore()

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
  <header
    class="h-11 shrink-0 flex items-center gap-3 px-2 border-b border-border bg-background text-xs"
  >
    <SidebarTrigger />
    <Separator orientation="vertical" class="h-4!" />

    <!-- Connection -->
    <Tooltip>
      <TooltipTrigger as-child>
        <span
          class="flex items-center gap-1.5"
          :class="wsStatus !== 'connected' && 'text-warning font-medium'"
        >
          <StatusDot :status="connection.status" />
          {{ wsStatus === 'connected' ? 'Live' : connection.label }}
        </span>
      </TooltipTrigger>
      <TooltipContent>Event stream: {{ connection.label }}</TooltipContent>
    </Tooltip>

    <!-- Nodes -->
    <button class="flex items-center gap-1.5 hover:text-foreground text-muted-foreground" @click="router.push('/nodes')">
      <StatusDot :status="healthy === nodes.nodes.length ? 'ok' : 'warn'" />
      <span class="num">{{ healthy }}/{{ nodes.nodes.length }}</span> nodes
      <span
        v-if="nodes.isController"
        class="ml-1 rounded-sm border border-border px-1 text-[10px] uppercase tracking-wide"
      >controller</span>
    </button>

    <!-- Recording -->
    <button
      class="flex items-center gap-1.5"
      :class="live ? 'text-tally font-semibold' : 'text-muted-foreground hover:text-foreground'"
      @click="router.push('/multiview')"
    >
      <StatusDot :status="live ? 'tally' : 'off'" />
      <template v-if="live"><span class="num">{{ live }}</span> recording</template>
      <template v-else>Idle</template>
    </button>

    <!-- Lowest storage -->
    <Tooltip v-if="lowest">
      <TooltipTrigger as-child>
        <span
          class="hidden md:flex items-center gap-1.5"
          :class="{
            'text-muted-foreground': lowest.status === 'ok',
            'text-warning': lowest.status === 'warn',
            'text-destructive font-medium': lowest.status === 'error',
          }"
        >
          <HardDrive class="size-3.5" />
          <span class="num">{{ formatBytes(lowest.volume.available_bytes) }}</span> free
        </span>
      </TooltipTrigger>
      <TooltipContent>
        Lowest free space: <span class="num">{{ lowest.volume.mount_point }}</span>
        on {{ nodes.nameOf(lowest.nodeId) }} ({{ Math.round(lowest.pct * 100) }}% free)
      </TooltipContent>
    </Tooltip>

    <div class="flex-1" />

    <button
      class="flex items-center gap-2 h-7 rounded-md border border-border bg-input/20 px-2 text-muted-foreground hover:text-foreground hover:bg-input/40 w-56 max-w-[40vw]"
      @click="$emit('command')"
    >
      <Search class="size-3.5" />
      <span class="flex-1 text-left truncate">Search or run a command…</span>
      <Kbd>Ctrl K</Kbd>
    </button>

    <span class="num text-muted-foreground w-[8ch] text-right pr-1">{{ clock }}</span>
  </header>
</template>
