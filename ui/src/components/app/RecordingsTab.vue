<script setup lang="ts">
import { computed } from 'vue'
import { useNow } from '@vueuse/core'
import { formatDuration } from '@/lib/format'
import { useNodesStore } from '@/stores/nodes'
import { usePresetsStore } from '@/stores/presets'
import { outputFiles, useRecordingsStore, type RecordingSession } from '@/stores/recordings'
import { useSourcesStore } from '@/stores/sources'
import { sourceKey } from '@/composables/useApi'
import StatusDot from '@/components/common/StatusDot.vue'
import CopyButton from '@/components/common/CopyButton.vue'

const recordings = useRecordingsStore()
const sources = useSourcesStore()
const nodes = useNodesStore()
const presets = usePresetsStore()
const now = useNow({ interval: 1000 })

/** Live sessions first, then the rest newest first. */
const rows = computed(() =>
  [...recordings.sessions].sort((a, b) => {
    if ((a.status === 'active') !== (b.status === 'active')) return a.status === 'active' ? -1 : 1
    return b.started_at.localeCompare(a.started_at)
  }),
)

function sourceName(s: RecordingSession) {
  return sources.sources.find((x) => x.key === sourceKey(s.node_id, s.source_id))?.display_name ?? s.source_id
}

function presetName(s: RecordingSession) {
  if (s.preset_id === 'default' || !s.preset_id) return 'H.264 (default)'
  return presets.presets.find((p) => p.id === s.preset_id)?.name ?? s.preset_id
}

function duration(s: RecordingSession) {
  const end = s.stopped_at ? new Date(s.stopped_at).getTime() : now.value.getTime()
  return formatDuration(end - new Date(s.started_at).getTime())
}

function started(s: RecordingSession) {
  return new Date(s.started_at).toLocaleString([], { hour12: false, month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', second: '2-digit' })
}

const statusDot = { active: 'tally', stopped: 'off', error: 'error' } as const
</script>

<template>
  <div class="h-full overflow-y-auto">
    <p v-if="rows.length === 0" class="px-3 py-2 text-xs text-muted-foreground">
      Recording sessions from every node appear here.
    </p>
    <table v-else class="w-full text-xs">
      <thead class="sticky top-0 bg-card text-muted-foreground text-left">
        <tr class="[&>th]:font-medium [&>th]:px-3 [&>th]:py-1.5 border-b border-border">
          <th class="w-6"></th>
          <th>Source</th>
          <th v-if="nodes.nodes.length > 1">Node</th>
          <th>Preset</th>
          <th>Started</th>
          <th class="text-right">Duration</th>
          <th>Outputs</th>
        </tr>
      </thead>
      <tbody>
        <tr
          v-for="s in rows"
          :key="`${s.node_id}/${s.id}`"
          class="[&>td]:px-3 [&>td]:py-1 border-b border-border/60 align-top hover:bg-accent/40"
        >
          <td class="pt-2"><StatusDot :status="statusDot[s.status]" /></td>
          <td class="font-medium">{{ sourceName(s) }}</td>
          <td v-if="nodes.nodes.length > 1" class="text-muted-foreground">{{ nodes.nameOf(s.node_id) }}</td>
          <td class="text-muted-foreground">{{ presetName(s) }}</td>
          <td class="num text-muted-foreground whitespace-nowrap">{{ started(s) }}</td>
          <td class="num text-right" :class="s.status === 'active' && 'text-tally font-medium'">{{ duration(s) }}</td>
          <td class="min-w-0">
            <div v-for="(p, i) in s.output_paths" :key="p" class="min-w-0">
              <div class="flex items-center gap-1 min-w-0">
                <span class="num truncate text-muted-foreground" :title="outputFiles(s, i)[0]">{{ outputFiles(s, i)[0] }}</span>
                <CopyButton :value="outputFiles(s, i)[0]!" />
                <span
                  v-if="s.dropped_frames[i]"
                  class="num shrink-0 text-warning"
                  title="Video frames dropped because this output's encoder couldn't keep up"
                >
                  {{ s.dropped_frames[i]!.toLocaleString() }} dropped
                </span>
              </div>
              <!-- A split output's later files. -->
              <details v-if="outputFiles(s, i).length > 1" class="group">
                <summary class="cursor-pointer select-none text-muted-foreground hover:text-foreground list-none">
                  <span class="group-open:hidden">+ {{ outputFiles(s, i).length - 1 }} more files</span>
                  <span class="hidden group-open:inline">− hide files</span>
                </summary>
                <div v-for="f in outputFiles(s, i).slice(1)" :key="f" class="flex items-center gap-1 min-w-0 pl-2">
                  <span class="num truncate text-muted-foreground" :title="f">{{ f }}</span>
                  <CopyButton :value="f" />
                </div>
              </details>
            </div>
            <p v-if="s.error_message" class="text-destructive break-words">{{ s.error_message }}</p>
          </td>
        </tr>
      </tbody>
    </table>
  </div>
</template>
