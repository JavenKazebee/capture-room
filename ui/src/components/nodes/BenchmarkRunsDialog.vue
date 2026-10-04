<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { Trash2 } from '@lucide/vue'
import { errorMessage } from '@/composables/useApi'
import { formatRate } from '@/lib/format'
import { notifyError } from '@/lib/notify'
import { formatLabel, useCapacityStore } from '@/stores/capacity'
import { useNodesStore } from '@/stores/nodes'
import type { BenchmarkRunDto } from '@/types/generated/BenchmarkRunDto'
import type { BenchmarkStatus } from '@/types/generated/BenchmarkStatus'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import KeyValueList from '@/components/common/KeyValueList.vue'

/** A node's benchmark runs: a list, and the selected run's steps. Mount it with `v-if`. */
const props = defineProps<{ nodeId: string; selectId?: string }>()
const emit = defineEmits<{ close: [] }>()

/** Length of a quick check (`QUICK_SECS` on the node). */
const QUICK_SECS = 5

const store = useCapacityStore()
const nodes = useNodesStore()

const loadError = ref<string | null>(null)
const selectedId = ref<string | null>(props.selectId ?? null)

const runs = computed(() => store.runs.get(props.nodeId) ?? [])
const selected = computed(() => runs.value.find((r) => r.id === selectedId.value) ?? runs.value[0] ?? null)

onMounted(async () => {
  try {
    await store.loadRuns(props.nodeId)
  } catch (e) {
    loadError.value = errorMessage(e, "Couldn't load benchmarks")
  }
})

const STATUS: Record<BenchmarkStatus, { label: string; variant: 'default' | 'secondary' | 'outline' | 'destructive' }> = {
  running: { label: 'Running', variant: 'default' },
  completed: { label: 'Completed', variant: 'secondary' },
  cancelled: { label: 'Cancelled', variant: 'outline' },
  error: { label: 'Failed', variant: 'destructive' },
}

function when(iso: string) {
  return new Date(iso).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' })
}

function result(run: BenchmarkRunDto) {
  if (run.status === 'running') return `${run.feeds_running} feed${run.feeds_running === 1 ? '' : 's'} running`
  if (run.status !== 'completed') return '—'
  const atLimit = run.sustainable_feeds >= run.max_feeds
  return `${atLimit ? '≥ ' : ''}${run.sustainable_feeds} feed${run.sustainable_feeds === 1 ? '' : 's'}`
}

const details = computed(() => {
  const r = selected.value
  if (!r) return []
  return [
    { label: 'Preset', value: r.preset_name ?? 'H.264 (default)' },
    {
      label: 'Outputs',
      value: r.outputs.map((o, i) => `${o.name}${r.encoders[i] ? ` (${r.encoders[i]})` : ''}`).join(', '),
    },
    { label: 'Footage', value: r.media_path, mono: true, copy: true },
    { label: 'Format', value: `${formatLabel(r.media)}${r.media.audio_channels ? `, ${r.media.audio_channels} ch audio` : ''}` },
    { label: 'Settings', value: `up to ${r.max_feeds} feeds, ${r.step_secs} s steps, ${r.drop_threshold_pct}% drop limit` },
    { label: 'Started', value: when(r.started_at) },
  ]
})

// ── Delete ──────────────────────────────────────────────────────────────────

const confirmDelete = ref(false)

async function remove() {
  const run = selected.value
  if (!run) return
  try {
    await store.remove(props.nodeId, run.id)
    selectedId.value = null
  } catch (e) {
    notifyError("Couldn't delete the benchmark", e, props.nodeId)
  }
}
</script>

<template>
  <Dialog :open="true" @update:open="(v) => !v && emit('close')">
    <DialogContent class="sm:max-w-5xl max-h-[85vh] flex flex-col gap-3">
      <DialogHeader>
        <DialogTitle>Benchmarks on {{ nodes.nameOf(nodeId) }}</DialogTitle>
        <DialogDescription>
          Each run adds feeds until frames drop. A recording setup's capacity comes from its latest completed run.
        </DialogDescription>
      </DialogHeader>

      <p v-if="loadError" class="text-xs text-destructive">{{ loadError }}</p>
      <p v-else-if="!runs.length" class="text-xs text-muted-foreground">No benchmarks have run on this node.</p>

      <div v-else class="grid grid-cols-[17rem_1fr] gap-4 min-h-0 flex-1">
        <!-- Runs -->
        <ul class="overflow-y-auto -mx-1 space-y-0.5 pr-1">
          <li v-for="run in runs" :key="run.id">
            <button
              type="button"
              class="w-full text-left rounded-md px-2 py-1.5 text-xs hover:bg-accent"
              :class="selected?.id === run.id && 'bg-accent'"
              @click="selectedId = run.id"
            >
              <span class="flex items-center gap-2">
                <span class="font-medium truncate">{{ run.preset_name ?? 'H.264 (default)' }}</span>
                <span class="flex-1" />
                <span class="num shrink-0">{{ result(run) }}</span>
              </span>
              <span class="flex items-center gap-1.5 text-muted-foreground">
                <span class="num">{{ formatLabel(run.media) }}</span>
                <span>·</span>
                <span>{{ when(run.started_at) }}</span>
                <span v-if="run.status !== 'completed'" class="ml-auto">{{ STATUS[run.status].label }}</span>
              </span>
            </button>
          </li>
        </ul>

        <!-- Selected run -->
        <section v-if="selected" class="min-w-0 overflow-y-auto space-y-3">
          <div class="flex items-center gap-2">
            <Badge :variant="STATUS[selected.status].variant">{{ STATUS[selected.status].label }}</Badge>
            <span class="text-sm font-medium num">{{ result(selected) }}</span>
            <div class="flex-1" />
            <Button
              variant="ghost"
              size="sm"
              class="h-7 gap-1.5 text-xs text-muted-foreground hover:text-destructive"
              :disabled="selected.status === 'running'"
              @click="confirmDelete = true"
            >
              <Trash2 class="size-3.5" /> Delete
            </Button>
          </div>
          <p v-if="selected.message" class="text-xs" :class="selected.status === 'error' && 'text-destructive'">
            {{ selected.message }}
          </p>
          <KeyValueList :items="details" />

          <Table v-if="selected.steps.length" class="text-xs">
            <TableHeader>
              <TableRow>
                <TableHead class="h-8">Feeds</TableHead>
                <TableHead class="h-8 text-right">Dropped</TableHead>
                <TableHead class="h-8 text-right" title="Frames the feeds didn't deliver: decoding fell behind">Source short</TableHead>
                <TableHead class="h-8 text-right" title="Frames dropped because an encoder couldn't keep up">Output drops</TableHead>
                <TableHead class="h-8 text-right">CPU</TableHead>
                <TableHead class="h-8 text-right">Memory</TableHead>
                <TableHead class="h-8 text-right">Writing</TableHead>
                <TableHead class="h-8">Result</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-for="(step, i) in selected.steps" :key="i">
                <TableCell class="py-1.5 num">
                  {{ step.feeds }}
                  <span v-if="step.quick" class="ml-1 font-sans text-muted-foreground">quick</span>
                </TableCell>
                <TableCell class="py-1.5 num text-right" :class="!step.passed && 'text-destructive'">
                  {{ step.drop_pct.toFixed(2) }}%
                </TableCell>
                <TableCell class="py-1.5 num text-right text-muted-foreground">
                  {{ step.source_shortfall }} / {{ step.expected_frames }}
                </TableCell>
                <TableCell class="py-1.5 num text-right text-muted-foreground">{{ step.output_dropped }}</TableCell>
                <TableCell class="py-1.5 num text-right">{{ Math.round(step.cpu_pct) }}%</TableCell>
                <TableCell class="py-1.5 num text-right">{{ Math.round(step.memory_pct) }}%</TableCell>
                <TableCell class="py-1.5 num text-right">{{ formatRate(step.write_bytes_per_sec) }}</TableCell>
                <TableCell class="py-1.5">
                  <span v-if="step.passed" class="text-muted-foreground">Kept up</span>
                  <span v-else-if="step.quick && selected.steps[i + 1]?.feeds === step.feeds" class="text-muted-foreground">
                    Too close: measured in full
                  </span>
                  <span v-else-if="selected.steps[i + 1]?.feeds === step.feeds" class="text-warning">Measured again</span>
                  <span v-else class="text-destructive">Dropped frames</span>
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
          <p class="text-[11px] text-muted-foreground">
            Steps run in order: feeds double with quick {{ QUICK_SECS }} s checks until frames drop, then full-length steps
            narrow down on the answer. A full step that drops frames is measured again before it counts. CPU and memory
            are for the whole machine.
          </p>
        </section>
      </div>
    </DialogContent>
  </Dialog>

  <ConfirmDialog v-model:open="confirmDelete" title="Delete this benchmark?" @confirm="remove">
    Its result stops counting toward this node's capacity.
  </ConfirmDialog>
</template>
