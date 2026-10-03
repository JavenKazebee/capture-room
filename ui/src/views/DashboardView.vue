<script setup lang="ts">
import { useSourcesStore } from '@/stores/sources'
import { useRecordingsStore } from '@/stores/recordings'
import { wsStatus } from '@/composables/useWebSocket'
import FeedCard from '@/components/FeedCard.vue'

const sources = useSourcesStore()
const recordings = useRecordingsStore()
</script>

<template>
  <div class="flex flex-col h-full">
    <!-- Main content -->
    <div class="flex-1 overflow-y-auto p-6">
      <h1 class="text-2xl font-semibold mb-6">Multiview</h1>

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
        class="grid gap-4 grid-cols-[repeat(auto-fill,minmax(min(100%,320px),1fr))]"
        :class="wsStatus !== 'connected' ? 'opacity-60' : ''"
      >
        <FeedCard
          v-for="source in sources.sources"
          :key="source.key"
          :source="source"
          :session="recordings.activeForSource(source.node_id, source.id)"
        />
      </div>
    </div>
  </div>
</template>
