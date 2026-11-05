<script setup lang="ts">
import { computed, reactive } from 'vue'
import { watchDebounced } from '@vueuse/core'
import { Gauge } from '@lucide/vue'
import { useCapacityStore } from '@/stores/capacity'
import { useNodesStore } from '@/stores/nodes'
import { useRecordingsStore } from '@/stores/recordings'
import { formatLabel } from '@/lib/sourceFormat'
import { plural } from '@/lib/format'
import type { FormatEstimateDto } from '@/types/generated/FormatEstimateDto'
import type { PresetEstimateDto } from '@/types/generated/PresetEstimateDto'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'

/** The preset editor's estimate of what recording with `outputs` costs per source, on each node it records on. */
const props = defineProps<{ outputs: PresetOutputInput[]; nodeIds: string[] }>()

const capacity = useCapacityStore()
const nodes = useNodesStore()
const recordings = useRecordingsStore()

/** The nodes this preset records on, else every reachable one: a new preset could go anywhere. */
const shown = computed(() => {
  const reachable = nodes.reachable.map((n) => n.id)
  const used = reachable.filter((id) => props.nodeIds.includes(id))
  return used.length ? used : reachable
})

/** Per node, the latest estimate; `null` when it failed. */
const estimates = reactive(new Map<string, PresetEstimateDto | null>())

// Recordings starting or stopping change the room left now.
watchDebounced(
  () => [JSON.stringify(props.outputs), shown.value.join(), recordings.activeSessions.length] as const,
  () => {
    for (const id of shown.value) {
      capacity
        .estimate(id, props.outputs)
        .then((e) => estimates.set(id, e))
        .catch(() => estimates.set(id, null))
    }
  },
  { debounce: 400, immediate: true },
)

const rows = computed(() => shown.value.map((id) => ({ id, name: nodes.nameOf(id), estimate: estimates.get(id) })))

function formatName(f: FormatEstimateDto) {
  const label = f.format ? formatLabel(f.format) : 'its sources'
  return f.sources > 1 ? `${label} (${plural(f.sources, 'source')})` : label
}

/** `about 25% each · fits about 4 · 2 more now`, or why there's no number. */
function costText(f: FormatEstimateDto, load: number) {
  const feeds = f.feeds
  if (feeds == null) return null
  if (feeds < 1) return `can't keep up with one source (${Math.round(100 / Math.max(feeds, 0.5))}% each)`
  const about = f.estimated ? 'about ' : ''
  const parts = [`${about}${Math.max(1, Math.round(100 / feeds))}% each`, `fits ${about}${Math.floor(feeds)}`]
  const free = Math.floor(Math.max(0, 1 - load) * feeds)
  if (load > 0.005) parts.push(free ? `${free} more now` : 'no room now')
  return parts.join(' · ')
}

function costClass(f: FormatEstimateDto, load: number) {
  if (f.feeds == null) return 'text-muted-foreground'
  if (f.feeds < 1 || (1 - load) * f.feeds < 1) return 'text-destructive'
  return ''
}
</script>

<template>
  <div v-if="rows.length" class="rounded-md border border-border px-3 py-2 space-y-1.5">
    <h2 class="flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
      <Gauge class="size-3.5" /> Capacity
    </h2>
    <ul class="space-y-1 text-xs">
      <li v-for="r in rows" :key="r.id" class="flex flex-wrap gap-x-2">
        <span class="font-medium">{{ r.name }}</span>
        <span v-if="r.estimate === undefined" class="text-muted-foreground">…</span>
        <span v-else-if="r.estimate === null" class="text-muted-foreground">couldn't estimate</span>
        <span v-else-if="!r.estimate.benchmarked" class="text-muted-foreground">
          not benchmarked: run one from its card on the Nodes page
        </span>
        <template v-else-if="r.estimate.formats.every((f) => f.feeds == null)">
          <span class="text-muted-foreground">no benchmark of these outputs yet</span>
        </template>
        <template v-else>
          <span
            v-for="(f, i) in r.estimate.formats"
            :key="i"
            :class="costClass(f, r.estimate.load)"
            :title="f.estimated ? 'Scaled from a benchmark of another format, so approximate' : undefined"
          >
            {{ formatName(f) }}: {{ costText(f, r.estimate.load) ?? 'no benchmark of these outputs' }}
          </span>
        </template>
      </li>
    </ul>
    <p class="text-[11px] text-muted-foreground">
      From each node's benchmarks of these outputs; names, folders and splits don't change the work.
    </p>
  </div>
</template>
