<script setup lang="ts">
import { computed } from 'vue'
import { useNow } from '@vueuse/core'
import { useRecordDeskStore } from '@/stores/recordDesk'
import { outputFiles, useRecordingsStore, type RecordingSession } from '@/stores/recordings'
import { usePresetsStore, blankLeg, presetLegs } from '@/stores/presets'
import { useNodesStore } from '@/stores/nodes'
import type { Source } from '@/stores/sources'
import { formatDuration } from '@/lib/format'
import { CODECS, CONTAINERS, chromaLabel, framerateLabel, hasBitrate, hasChroma } from '@/lib/codecs'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'
import { fpsLabel, resolutionLabel } from '@/lib/sourceFormat'
import { Badge } from '@/components/ui/badge'
import KeyValueList from '@/components/common/KeyValueList.vue'
import CopyButton from '@/components/common/CopyButton.vue'
import StatusDot from '@/components/common/StatusDot.vue'

/** A feed's details for its tile's info popover: what's being written, the outputs, the source, recent takes. */
const props = defineProps<{ source: Source }>()

const desk = useRecordDeskStore()
const recordings = useRecordingsStore()
const presets = usePresetsStore()
const nodes = useNodesStore()
const now = useNow({ interval: 1000 })

const session = computed(() => recordings.activeForSource(props.source.node_id, props.source.id))

/** The outputs Record would send right now. */
const legs = computed(() => {
  const p = presets.presets.find((p) => p.id === desk.presetIdOf(props.source.key))
  return p ? presetLegs(p) : [blankLeg()]
})

function legFacts(leg: PresetOutputInput) {
  return [
    { label: 'Format', value: `${CODECS[leg.codec]} ${CONTAINERS[leg.container]}` },
    { label: 'Resolution', value: leg.resolution ?? 'source', mono: !!leg.resolution },
    { label: 'Frame rate', value: leg.framerate ? `${framerateLabel(leg.framerate)} fps` : 'source', mono: !!leg.framerate },
    ...(hasBitrate(leg.codec) ? [{ label: 'Bitrate', value: leg.bitrate_kbps ? `${leg.bitrate_kbps} kbps` : 'auto', mono: !!leg.bitrate_kbps }] : []),
    ...(hasChroma(leg.codec) ? [{ label: 'Chroma', value: chromaLabel(leg.chroma), mono: true }] : []),
    { label: 'Path', value: leg.path_template, mono: true },
  ]
}

const sourceFacts = computed(() => {
  const s = props.source
  const c = s.capabilities
  return [
    { label: 'Node', value: nodes.labelOf(s.node_id) },
    { label: 'ID', value: s.id, mono: true, copy: true },
    { label: 'Resolution', value: c ? resolutionLabel(c) : 'negotiated on connect', mono: !!c },
    { label: 'Frame rate', value: c ? fpsLabel(c.max_framerate) : '—', mono: !!c },
    { label: 'Audio', value: c ? `${c.audio_channels} ch` : '—', mono: !!c },
  ]
})

const history = computed(() =>
  recordings.sessions
    .filter((r) => r.node_id === props.source.node_id && r.source_id === props.source.id && r.status !== 'active')
    .sort((a, b) => b.started_at.localeCompare(a.started_at))
    .slice(0, 5),
)

function elapsed(r: RecordingSession) {
  const end = r.stopped_at ? new Date(r.stopped_at).getTime() : now.value.getTime()
  return formatDuration(end - new Date(r.started_at).getTime())
}

function startedAt(r: RecordingSession) {
  return new Date(r.started_at).toLocaleString([], { hour12: false, month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', second: '2-digit' })
}
</script>

<template>
  <div class="space-y-3">
    <div class="flex items-center gap-2 min-w-0">
      <h2 class="text-sm font-semibold truncate">{{ source.display_name }}</h2>
      <Badge variant="secondary" class="uppercase">{{ source.source_type }}</Badge>
      <div class="flex-1" />
      <span v-if="session" class="flex items-center gap-1.5 text-tally font-medium">
        <StatusDot status="tally" /> Recording <span class="num">{{ elapsed(session) }}</span>
      </span>
      <span v-else class="flex items-center gap-1.5 text-muted-foreground">
        <StatusDot :status="source.error ? 'error' : 'off'" /> {{ source.error ? 'Failed' : 'Idle' }}
      </span>
    </div>
    <p v-if="source.error" class="text-destructive break-words">{{ source.error }}</p>

    <!-- Live: what's being written right now -->
    <section v-if="session" class="space-y-1.5">
      <h3 class="section-title">Writing</h3>
      <div v-for="(p, i) in session.output_paths" :key="p" class="flex items-center gap-1 min-w-0">
        <!-- The file being written now: the latest of a split output's. -->
        <span class="num truncate text-muted-foreground" :title="outputFiles(session, i).join('\n')">
          {{ outputFiles(session, i).at(-1) }}
        </span>
        <CopyButton :value="outputFiles(session, i).at(-1)!" />
        <span
          v-if="outputFiles(session, i).length > 1"
          class="num shrink-0 rounded bg-muted px-1 text-[10px] text-muted-foreground"
          title="This output splits; it's writing this file number"
        >
          file {{ outputFiles(session, i).length }}
        </span>
        <span
          class="num ml-auto shrink-0"
          :class="session.dropped_frames[i] ? 'text-warning' : 'text-muted-foreground'"
          title="Video frames dropped because this output's encoder couldn't keep up"
        >
          {{ (session.dropped_frames[i] ?? 0).toLocaleString() }} dropped
        </span>
      </div>
      <p v-if="session.error_message" class="text-destructive break-words">Output failed: {{ session.error_message }}</p>
    </section>

    <div class="grid grid-cols-2 gap-x-5 gap-y-3 items-start">
      <section v-for="(leg, i) in legs" :key="i" class="space-y-1.5 min-w-0">
        <h3 class="section-title">{{ leg.name || (legs.length > 1 ? `Output ${i + 1}` : 'Output') }}</h3>
        <KeyValueList :items="legFacts(leg)" />
      </section>

      <section class="space-y-1.5 min-w-0">
        <h3 class="section-title">Source</h3>
        <KeyValueList :items="sourceFacts" />
      </section>

      <section v-if="history.length" class="space-y-1.5 min-w-0">
        <h3 class="section-title">Recent recordings</h3>
        <ul class="space-y-1">
          <li v-for="r in history" :key="r.id" class="flex items-center gap-2" :title="r.error_message ?? r.files.flat().join('\n')">
            <StatusDot :status="r.status === 'error' ? 'error' : 'off'" />
            <span class="num text-muted-foreground">{{ startedAt(r) }}</span>
            <span class="num ml-auto">{{ elapsed(r) }}</span>
          </li>
        </ul>
      </section>
    </div>
  </div>
</template>

<style scoped>
@reference "@/style.css";
.section-title {
  @apply text-[11px] font-semibold uppercase tracking-wider text-muted-foreground;
}
</style>
