<script setup lang="ts">
import { computed, ref } from 'vue'
import { useNow } from '@vueuse/core'
import { MousePointerClick, X } from '@lucide/vue'
import { useRecordDeskStore } from '@/stores/recordDesk'
import { useRecordingsStore, type RecordingSession } from '@/stores/recordings'
import { usePresetsStore, blankLeg, presetLegs } from '@/stores/presets'
import { useNodesStore } from '@/stores/nodes'
import { formatDuration } from '@/lib/format'
import { legSummary } from '@/lib/codecs'
import { fpsLabel, resolutionLabel } from '@/lib/sourceFormat'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import OptionSelect from '@/components/OptionSelect.vue'
import KeyValueList from '@/components/common/KeyValueList.vue'
import CopyButton from '@/components/common/CopyButton.vue'
import StatusDot from '@/components/common/StatusDot.vue'
import TallyBadge from '@/components/common/TallyBadge.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import { useThumbnail } from './useThumbnail'

defineEmits<{ close: [] }>()

const desk = useRecordDeskStore()
const recordings = useRecordingsStore()
const presets = usePresetsStore()
const nodes = useNodesStore()
const now = useNow({ interval: 1000 })

const source = computed(() => desk.focused)
const multi = computed(() => desk.selectedSources.length > 1)
const thumb = useThumbnail(source)

const session = computed(() => (source.value ? recordings.activeForSource(source.value.node_id, source.value.id) : null))
const busy = computed(() => !!source.value && desk.busy.has(source.value.key))

const presetId = computed({
  get: () => (source.value ? desk.presetIdOf(source.value.key) : 'default'),
  set: (id: string) => source.value && desk.setPreset(source.value.key, id),
})

/** The outputs Record would send right now. */
const legs = computed(() => {
  const p = presets.presets.find((p) => p.id === presetId.value)
  return p ? presetLegs(p) : [blankLeg()]
})

const sourceFacts = computed(() => {
  const s = source.value
  if (!s) return []
  const c = s.capabilities
  return [
    { label: 'Type', value: s.source_type.toUpperCase() },
    { label: 'Node', value: nodes.labelOf(s.node_id) },
    { label: 'ID', value: s.id, mono: true, copy: true },
    { label: 'Resolution', value: c ? resolutionLabel(c) : 'negotiated on connect', mono: !!c },
    { label: 'Frame rate', value: c ? fpsLabel(c.max_framerate) : '—', mono: !!c },
    { label: 'Audio', value: c ? `${c.audio_channels} ch` : '—', mono: !!c },
    { label: 'Timecode', value: s.timecode, mono: true },
  ]
})

const history = computed(() =>
  source.value
    ? recordings.sessions
        .filter((r) => r.node_id === source.value!.node_id && r.source_id === source.value!.id && r.status !== 'active')
        .sort((a, b) => b.started_at.localeCompare(a.started_at))
        .slice(0, 5)
    : [],
)

function elapsed(r: RecordingSession) {
  const end = r.stopped_at ? new Date(r.stopped_at).getTime() : now.value.getTime()
  return formatDuration(end - new Date(r.started_at).getTime())
}

function startedAt(r: RecordingSession) {
  return new Date(r.started_at).toLocaleString([], { hour12: false, month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', second: '2-digit' })
}

// ── Multi-select ──────────────────────────────────────────────────────────────

const liveSelected = computed(() => desk.selectedSources.filter((s) => desk.isLive(s)))
const idleSelected = computed(() => desk.selectedSources.filter((s) => !desk.isLive(s)))
const confirmStop = ref(false)
</script>

<template>
  <aside class="h-full flex flex-col min-h-0 bg-card border-l border-border">
    <div class="h-9 shrink-0 flex items-center gap-2 px-3 border-b border-border">
      <span class="text-xs font-semibold">Inspector</span>
      <span v-if="multi" class="text-xs text-muted-foreground">{{ desk.selectedSources.length }} selected</span>
      <div class="flex-1" />
      <button class="size-6 grid place-items-center rounded text-muted-foreground hover:text-foreground hover:bg-accent" title="Close inspector" @click="$emit('close')">
        <X class="size-3.5" />
      </button>
    </div>

    <!-- Several selected -->
    <div v-if="multi" class="flex-1 min-h-0 overflow-y-auto p-3 space-y-3">
      <div class="flex gap-2">
        <Button variant="outline" size="sm" class="flex-1 gap-1.5" :disabled="!idleSelected.length" @click="desk.bulk('start', idleSelected)">
          <span class="size-2 rounded-full bg-tally" /> Record {{ idleSelected.length }}
        </Button>
        <Button variant="outline" size="sm" class="flex-1 gap-1.5" :disabled="!liveSelected.length" @click="confirmStop = true">
          <span class="size-2 rounded-[1px] bg-tally" /> Stop {{ liveSelected.length }}
        </Button>
      </div>
      <ul class="divide-y divide-border rounded-md border border-border">
        <li
          v-for="s in desk.selectedSources"
          :key="s.key"
          class="flex items-center gap-2 px-2.5 py-1.5 text-xs hover:bg-accent/40 cursor-pointer"
          :class="desk.focusedKey === s.key && 'bg-accent/60'"
          @click="desk.focusedKey = s.key"
        >
          <StatusDot :status="desk.isLive(s) ? 'tally' : s.error ? 'error' : 'off'" />
          <span class="font-medium truncate flex-1">{{ s.display_name }}</span>
          <span class="text-muted-foreground truncate">
            {{ desk.presetOptions.find((o) => o.value === desk.presetIdOf(s.key))?.label }}
          </span>
        </li>
      </ul>
      <p class="text-xs text-muted-foreground">Each feed records with the preset chosen on its tile.</p>
      <ConfirmDialog
        v-model:open="confirmStop"
        :title="`Stop ${liveSelected.length} recording${liveSelected.length > 1 ? 's' : ''}?`"
        description="The selected feeds stop recording now. Files written so far are kept."
        confirm-label="Stop recording"
        @confirm="desk.bulk('stop', liveSelected)"
      />
    </div>

    <!-- One feed -->
    <div v-else-if="source" class="flex-1 min-h-0 overflow-y-auto">
      <div class="relative bg-black aspect-video">
        <img v-if="thumb.src.value && !source.error" :src="thumb.src.value" :alt="source.display_name" class="w-full h-full object-contain" @error="thumb.onError" />
        <div v-else class="absolute inset-0 grid place-items-center text-xs" :class="source.error ? 'text-destructive' : 'text-zinc-500'">
          {{ source.error ? 'Source failed' : 'No signal' }}
        </div>
        <div v-if="session" class="absolute top-2 left-2">
          <TallyBadge :duration="formatDuration(now.getTime() - new Date(session.started_at).getTime())" />
        </div>
      </div>

      <div class="p-3 space-y-4">
        <div>
          <div class="flex items-center gap-2">
            <h2 class="text-sm font-semibold truncate">{{ source.display_name }}</h2>
            <Badge variant="secondary" class="uppercase">{{ source.source_type }}</Badge>
          </div>
          <p v-if="source.error" class="mt-1 text-xs text-destructive break-words">{{ source.error }}</p>
        </div>

        <!-- Record -->
        <section class="space-y-2">
          <h3 class="section-title">Record</h3>
          <div class="flex gap-2">
            <OptionSelect v-model="presetId" :options="desk.presetOptions" :disabled="!!session || busy" class="h-8 flex-1 min-w-0" />
            <Button
              :variant="session ? 'default' : 'outline'"
              class="h-8 px-4 gap-1.5 shrink-0"
              :class="session && 'bg-tally text-tally-foreground hover:bg-tally/85'"
              :disabled="busy"
              @click="desk.toggle(source)"
            >
              <span v-if="session" class="size-2 rounded-[1px] bg-current" />
              <span v-else class="size-2 rounded-full bg-tally" />
              {{ session ? 'Stop' : 'Record' }}
            </Button>
          </div>
          <ol class="space-y-1.5">
            <li v-for="(leg, i) in legs" :key="i" class="rounded-md border border-border px-2.5 py-1.5 text-xs">
              <div class="font-medium">{{ leg.name || `Output ${i + 1}` }}</div>
              <div class="text-muted-foreground">{{ legSummary(leg) }}</div>
              <div class="num text-muted-foreground truncate" :title="leg.path_template">{{ leg.path_template }}</div>
            </li>
          </ol>
        </section>

        <!-- Live session -->
        <section v-if="session" class="space-y-2">
          <h3 class="section-title">Session</h3>
          <KeyValueList
            :items="[
              { label: 'Started', value: startedAt(session), mono: true },
              { label: 'Duration', value: elapsed(session), mono: true },
              { label: 'Session', value: session.id, mono: true, copy: true },
            ]"
          />
          <div class="space-y-1">
            <div v-for="p in session.output_paths" :key="p" class="flex items-center gap-1 min-w-0 text-xs">
              <span class="num truncate text-muted-foreground" :title="p">{{ p }}</span>
              <CopyButton :value="p" />
            </div>
          </div>
          <p v-if="session.error_message" class="text-xs text-destructive break-words">
            Output failed: {{ session.error_message }}
          </p>
        </section>

        <!-- Source -->
        <section class="space-y-2">
          <h3 class="section-title">Source</h3>
          <KeyValueList :items="sourceFacts" />
        </section>

        <!-- History -->
        <section v-if="history.length" class="space-y-2">
          <h3 class="section-title">Recent recordings</h3>
          <ul class="space-y-1 text-xs">
            <li v-for="r in history" :key="r.id" class="flex items-center gap-2" :title="r.error_message ?? r.output_paths.join('\n')">
              <StatusDot :status="r.status === 'error' ? 'error' : 'off'" />
              <span class="num text-muted-foreground">{{ startedAt(r) }}</span>
              <span class="num ml-auto">{{ elapsed(r) }}</span>
            </li>
          </ul>
        </section>
      </div>
    </div>

    <!-- Nothing selected -->
    <div v-else class="flex-1 grid place-items-center p-6 text-center">
      <div class="space-y-2 text-xs text-muted-foreground max-w-56">
        <MousePointerClick class="size-5 mx-auto" />
        <p class="text-sm text-foreground">Select a feed to inspect it</p>
        <p>Ctrl/⌘-click or Shift-click to select several, or press Ctrl+A for all.</p>
      </div>
    </div>
  </aside>
</template>

<style scoped>
@reference "@/style.css";
.section-title {
  @apply text-[11px] font-semibold uppercase tracking-wider text-muted-foreground;
}
</style>
