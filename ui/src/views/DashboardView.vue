<script setup lang="ts">
import { onMounted } from 'vue'
import { useSourcesStore } from '@/stores/sources'
import { useRecordingsStore } from '@/stores/recordings'
import { wsStatus } from '@/composables/useWebSocket'
import { useApi } from '@/composables/useApi'
import FeedCard from '@/components/FeedCard.vue'
import { WifiOff } from '@lucide/vue'

const sources = useSourcesStore()
const recordings = useRecordingsStore()
const { api } = useApi()

onMounted(async () => {
  const [fetchedSources, fetchedRecordings] = await Promise.all([
    api('/sources').catch(() => []),
    api('/recordings').catch(() => []),
  ])

  for (const s of fetchedSources as ReturnType<typeof sources.upsert>[]) {
    sources.upsert(s as Parameters<typeof sources.upsert>[0])
  }

  for (const r of fetchedRecordings as Parameters<typeof recordings.upsert>[0][]) {
    recordings.upsert(r)
  }
})
</script>

<template>
  <div class="flex flex-col h-full">
    <!-- Reconnecting banner -->
    <div
      v-if="wsStatus !== 'connected'"
      class="flex items-center gap-2 px-4 py-2 bg-yellow-500/15 border-b border-yellow-500/30 text-yellow-700 dark:text-yellow-400 text-sm"
    >
      <WifiOff class="w-4 h-4 shrink-0" />
      <span>
        {{
          wsStatus === 'connecting'
            ? 'Connecting to server…'
            : 'Disconnected — reconnecting…'
        }}
      </span>
    </div>

    <!-- Main content -->
    <div class="flex-1 overflow-y-auto p-6">
      <h1 class="text-2xl font-semibold mb-6">Dashboard</h1>

      <!-- Empty state -->
      <div
        v-if="sources.sources.length === 0"
        class="text-center text-muted-foreground py-24"
      >
        <p class="text-lg font-medium mb-1">No sources found</p>
        <p class="text-sm">Make sure the capture node is running and has sources available.</p>
      </div>

      <!-- Feed grid -->
      <div
        v-else
        class="grid gap-4"
        :class="wsStatus !== 'connected' ? 'opacity-60 pointer-events-none' : ''"
        style="grid-template-columns: repeat(3, minmax(0, 1fr))"
      >
        <FeedCard
          v-for="source in sources.sources"
          :key="source.id"
          :source="source"
          :session="recordings.activeForSource(source.id)"
        />
      </div>
    </div>
  </div>
</template>
