<script setup lang="ts">
import { computed } from 'vue'
import { useRouter } from 'vue-router'
import { ArrowRight, Flag, X } from '@lucide/vue'
import { useSecondClock } from '@/composables/useSecondClock'
import { clockTime, elapsedMs, formatDuration, plural } from '@/lib/format'
import { RECORDING_STATUS, recordingFormats } from '@/lib/recordingStatus'
import { useNodesStore } from '@/stores/nodes'
import { outputFiles, totalDropped, useRecordingsStore, type RecordingSession } from '@/stores/recordings'
import StatusDot from '@/components/common/StatusDot.vue'
import { useMarkerExport } from '@/composables/useMarkerExport'

/**
 * Recordings started since `since` (when this tab first opened the Record page,
 * or was last cleared), newest first, so a take can be checked without leaving.
 */
const since = defineModel<string>('since', { required: true })
const emit = defineEmits<{ close: [] }>()

const recordings = useRecordingsStore()
const nodes = useNodesStore()
const router = useRouter()
const now = useSecondClock()

// Node clocks can run a little behind this browser's: a take started just
// after the session began mustn't fall before it.
const SKEW_MS = 30_000

const rows = computed(() => {
  const from = new Date(since.value).getTime() - SKEW_MS
  return recordings.sessions
    .filter((s) => s.status === 'active' || new Date(s.started_at).getTime() >= from)
    .sort((a, b) => b.started_at.localeCompare(a.started_at))
})

const key = (s: RecordingSession) => `${s.node_id}/${s.id}`

const { exportMarkers } = useMarkerExport()
const markerCount = computed(() => new Set(rows.value.flatMap((s) => s.markers.map((m) => m.press_id))).size)

function fileCount(s: RecordingSession) {
  return s.output_paths.reduce((n, _, i) => n + outputFiles(s, i).length, 0)
}

function open(s: RecordingSession) {
  router.push({ path: '/recordings', query: { select: key(s) } })
}

/** Start the list over from now; the recordings themselves stay in Recordings. */
function clear() {
  since.value = new Date().toISOString()
}
</script>

<template>
  <aside class="w-72 shrink-0 border-l border-border flex flex-col min-h-0 bg-background" aria-label="Recordings this session">
    <div class="flex items-center gap-2 h-10 px-3 border-b border-border shrink-0">
      <h2 class="text-xs font-semibold">This session</h2>
      <span class="num text-xs text-muted-foreground">{{ rows.length }}</span>
      <span class="flex-1" />
      <button
        v-if="rows.some((s) => s.status !== 'active')"
        class="text-xs text-muted-foreground hover:text-foreground"
        title="Start the list over; the recordings stay in Recordings"
        @click="clear"
      >
        Clear
      </button>
      <button
        class="size-6 grid place-items-center rounded text-muted-foreground hover:text-foreground hover:bg-accent"
        aria-label="Close this session's recordings"
        title="Close"
        @click="emit('close')"
      >
        <X class="size-3.5" />
      </button>
    </div>

    <ol v-if="rows.length" class="flex-1 min-h-0 overflow-y-auto p-2 space-y-1">
      <li v-for="s in rows" :key="key(s)">
        <button
          class="w-full text-left rounded-md px-2.5 py-2 hover:bg-accent focus-visible:bg-accent outline-none flex flex-col gap-0.5"
          title="Open in Recordings"
          @click="open(s)"
        >
          <span class="flex items-center gap-1.5 min-w-0">
            <StatusDot :status="RECORDING_STATUS[s.status].dot" />
            <span class="text-sm font-medium truncate">{{ recordings.sourceNameOf(s) }}</span>
            <span class="flex-1" />
            <span class="num text-xs shrink-0" :class="s.status === 'active' ? 'text-tally font-medium' : 'text-muted-foreground'">
              {{ formatDuration(elapsedMs(s.started_at, s.stopped_at, now)) }}
            </span>
          </span>
          <span class="flex items-center gap-1.5 text-xs text-muted-foreground min-w-0">
            <span class="num shrink-0">{{ clockTime(s.started_at) }}</span>
            <span>·</span>
            <span class="truncate">{{ recordingFormats(s) }}</span>
            <template v-if="s.status !== 'active'">
              <span>·</span>
              <span class="shrink-0" :class="fileCount(s) === 0 && 'text-warning'">
                {{ plural(fileCount(s), 'file') }}
              </span>
            </template>
          </span>
          <span v-if="nodes.nodes.length > 1" class="text-xs text-muted-foreground truncate">{{ nodes.nameOf(s.node_id) }}</span>
          <span v-if="s.markers.length" class="flex items-center gap-1 text-xs text-muted-foreground num">
            <Flag class="size-3" /> {{ plural(s.markers.length, 'marker') }}
          </span>
          <span v-if="totalDropped(s)" class="text-xs text-warning num">{{ totalDropped(s).toLocaleString() }} frames dropped</span>
          <span v-if="s.error_message" class="text-xs text-destructive line-clamp-2">{{ s.error_message }}</span>
        </button>
      </li>
    </ol>
    <p v-else class="flex-1 px-4 py-8 text-center text-xs text-muted-foreground">
      Recordings started from here on appear as they start.
    </p>

    <button
      v-if="markerCount"
      class="flex items-center justify-center gap-1.5 h-9 shrink-0 border-t border-border text-xs font-medium hover:bg-accent"
      title="Download these recordings' markers as an EDL for Resolve (Timeline Markers from EDL)"
      @click="exportMarkers(rows)"
    >
      <Flag class="size-3.5" /> Export {{ plural(markerCount, 'marker') }} for Resolve
    </button>
    <RouterLink
      to="/recordings"
      class="flex items-center justify-center gap-1 h-9 shrink-0 border-t border-border text-xs text-muted-foreground hover:text-foreground"
    >
      All recordings <ArrowRight class="size-3" />
    </RouterLink>
  </aside>
</template>
