<script setup lang="ts">
import { computed } from 'vue'
import { RouterLink, useRouter } from 'vue-router'
import { useNow } from '@vueuse/core'
import { HardDrive, Search } from '@lucide/vue'
import { wsStatus } from '@/composables/useWebSocket'
import { formatBytes } from '@/lib/format'
import { useNodesStore } from '@/stores/nodes'
import { useRecordingsStore } from '@/stores/recordings'
import { useStorageStore } from '@/stores/storage'
import { Kbd } from '@/components/ui/kbd'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import StatusDot from '@/components/common/StatusDot.vue'
import { workspaces } from './workspaces'

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
</script>

<template>
  <header class="h-11 shrink-0 flex items-center gap-1 px-3 border-b border-border bg-sidebar">
    <!-- Identity -->
    <RouterLink to="/record" class="flex items-center gap-2 pr-3 mr-1 border-r border-border h-6">
      <span class="size-5 rounded-[5px] bg-zinc-900 ring-1 ring-white/15 grid place-items-center">
        <span class="size-2 rounded-full bg-brand" />
      </span>
      <span class="text-sm font-semibold tracking-tight">Capture Room</span>
    </RouterLink>

    <!-- Workspaces -->
    <nav class="flex items-center gap-0.5 h-full">
      <RouterLink
        v-for="w in workspaces"
        :key="w.to"
        :to="w.to"
        class="ws-tab"
        active-class="ws-tab-active"
      >
        {{ w.label }}
      </RouterLink>
    </nav>

    <div class="flex-1" />

    <!-- Global status -->
    <div class="flex items-center gap-1.5">
      <button
        class="chip w-52 text-muted-foreground hover:text-foreground"
        @click="$emit('command')"
      >
        <Search class="size-3.5" />
        <span class="flex-1 text-left">Search or run…</span>
        <Kbd>Ctrl K</Kbd>
      </button>

      <Tooltip v-if="wsStatus !== 'connected'">
        <TooltipTrigger as-child>
          <span class="chip bg-warning/15 text-warning font-medium border-warning/30">
            <StatusDot status="warn" />
            {{ wsStatus === 'connecting' ? 'Connecting…' : 'Reconnecting…' }}
          </span>
        </TooltipTrigger>
        <TooltipContent>Lost the live event stream; the data shown may be stale.</TooltipContent>
      </Tooltip>

      <button class="chip hover:bg-accent" @click="router.push('/setup/nodes')">
        <StatusDot :status="healthy === nodes.nodes.length ? 'ok' : 'warn'" />
        <span class="num">{{ healthy }}/{{ nodes.nodes.length }}</span>
        <span class="text-muted-foreground">nodes</span>
      </button>

      <Tooltip v-if="lowest">
        <TooltipTrigger as-child>
          <button
            class="chip hover:bg-accent"
            :class="{
              'text-warning border-warning/30': lowest.status === 'warn',
              'text-destructive border-destructive/40 font-medium': lowest.status === 'error',
            }"
            @click="router.push('/setup/nodes')"
          >
            <HardDrive class="size-3.5 text-muted-foreground" />
            <span class="num">{{ formatBytes(lowest.volume.available_bytes) }}</span>
          </button>
        </TooltipTrigger>
        <TooltipContent>
          Lowest free space: <span class="num">{{ lowest.volume.mount_point }}</span>
          on {{ nodes.nameOf(lowest.nodeId) }} ({{ Math.round(lowest.pct * 100) }}% free)
        </TooltipContent>
      </Tooltip>

      <button
        class="chip min-w-[4.5rem] justify-center font-semibold"
        :class="live ? 'bg-tally text-tally-foreground border-transparent' : 'text-muted-foreground hover:bg-accent'"
        @click="router.push('/record')"
      >
        <span v-if="live" class="size-1.5 rounded-full bg-current animate-tally" />
        <template v-if="live"><span class="num">{{ live }}</span> REC</template>
        <template v-else>No REC</template>
      </button>

      <span class="num text-sm font-medium pl-2 w-[8.5ch] text-right">{{ clock }}</span>
    </div>
  </header>
</template>

<style scoped>
@reference "@/style.css";
.ws-tab {
  @apply relative h-full flex items-center px-3 text-sm text-muted-foreground hover:text-foreground;
}
.ws-tab-active {
  @apply text-foreground font-medium;
}
.ws-tab-active::after {
  content: '';
  @apply absolute inset-x-2 -bottom-px h-0.5 rounded-full bg-primary;
}
.chip {
  @apply h-7 flex items-center gap-1.5 px-2.5 rounded-md border border-border text-xs whitespace-nowrap;
}
</style>
