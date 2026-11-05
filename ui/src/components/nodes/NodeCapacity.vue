<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Check, Gauge, Loader2, X } from '@lucide/vue'
import { formatRate, plural } from '@/lib/format'
import { notifyError } from '@/lib/notify'
import { setupLabel, useCapacityStore } from '@/stores/capacity'
import { formatLabel } from '@/lib/sourceFormat'
import type { BenchmarkStepDto } from '@/types/generated/BenchmarkStepDto'
import type { NodeDto } from '@/types/generated/NodeDto'
import type { ProfileCapacityDto } from '@/types/generated/ProfileCapacityDto'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import BenchmarkRunsDialog from './BenchmarkRunsDialog.vue'
import BenchmarkDialog from './BenchmarkDialog.vue'
import LoadError from '@/components/common/LoadError.vue'

/** A node card's Capacity section: what each benchmarked setup sustains, current load and the running benchmark. */
const props = defineProps<{ node: NodeDto; recording: number }>()

const store = useCapacityStore()

/** The last good reading; `undefined` until one loads. */
const cap = computed(() => store.capacity.get(props.node.id))
const capError = computed(() => store.errors.get(props.node.id))
const running = computed(() => cap.value?.running ?? null)

// Load when the node is reachable, and again whenever its recordings change
// (they change its load).
watch(
  () => [props.node.healthy, props.recording] as const,
  ([healthy]) => healthy && store.load(props.node.id),
  { immediate: true },
)

const loadPct = computed(() => Math.round((cap.value?.load ?? 0) * 100))
const loadClass = computed(() => (loadPct.value > 100 ? 'bg-destructive' : loadPct.value > 80 ? 'bg-warning' : 'bg-primary'))

const SHOWN = 3
const profiles = computed(() => cap.value?.profiles ?? [])

function feedsLabel(p: ProfileCapacityDto) {
  const n = p.sustainable_feeds
  return `${p.at_limit ? '≥ ' : ''}${plural(n, 'feed')}`
}

function profileTitle(p: ProfileCapacityDto) {
  const lines = p.channel
    ? [`Playout channels: the program${p.encoders.length ? ` and ${p.encoders.join(', ')}` : ' alone'}`]
    : p.replay
    ? [`Replay buffers${p.encoders[0] ? ` via ${p.encoders[0]}` : ''}`]
    : p.outputs.map((o, i) => `${o.name}: ${o.codec}/${o.container}${p.encoders[i] ? ` via ${p.encoders[i]}` : ''}`)
  if (p.at_limit) lines.push('Every step kept up: run again with a higher feed limit to find the maximum.')
  return lines.join('\n')
}

/** Recordings, replay buffers and channels counted in the load. */
const active = computed(() =>
  cap.value ? cap.value.active_feeds + cap.value.active_buffers + cap.value.active_channels : 0,
)

/** `2 recording · 1 replay buffer · 1 channel` */
const activeLabel = computed(() => {
  const c = cap.value
  if (!c) return ''
  return [
    c.active_feeds ? `${c.active_feeds} recording` : '',
    c.active_buffers ? plural(c.active_buffers, 'replay buffer') : '',
    c.active_channels ? plural(c.active_channels, 'channel') : '',
  ]
    .filter(Boolean)
    .join(' · ')
})

/** The latest measurement of the feeds running now, if one has finished. */
const reading = computed(() => {
  const run = running.value
  const step = run?.steps.at(-1)
  return step && step.feeds === run!.feeds_running ? step : null
})

function stepTitle(s: BenchmarkStepDto) {
  return `${plural(s.feeds, 'feed')}, ${s.quick ? 'quick check' : 'full step'}: ${s.passed ? 'kept up' : 'fell behind'}
${s.drop_pct.toFixed(2)}% dropped · CPU ${Math.round(s.cpu_pct)}%`
}
const cancelling = ref(false)

async function cancel() {
  const run = running.value
  if (!run || cancelling.value) return
  cancelling.value = true
  try {
    await store.cancel(props.node.id, run.id)
  } catch (e) {
    notifyError("Couldn't cancel the benchmark", e, props.node.id)
  } finally {
    cancelling.value = false
  }
}

const starting = ref(false)
const history = ref<{ selectId?: string } | null>(null)

const startBlocked = computed(() => {
  if (running.value) return 'A benchmark is running.'
  if (props.recording) return 'Stop the recordings on this node first: they would skew the result.'
  if (cap.value?.active_buffers) return 'Turn off the replay buffers on this node first (Replay › Buffers): they would skew the result.'
  return null
})
</script>

<template>
  <section class="px-4 py-3 space-y-2 border-t border-border">
    <div class="flex items-center gap-2 h-5">
      <h3 class="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">Capacity</h3>
      <div class="flex-1" />
      <Button
        v-if="node.healthy && cap"
        variant="link"
        size="sm"
        class="h-5 px-0 text-xs text-muted-foreground"
        @click="history = {}"
      >
        History
      </Button>
      <Tooltip :disabled="!startBlocked">
        <TooltipTrigger as-child>
          <!-- A disabled button gets no pointer events; the span keeps the tooltip working. -->
          <span tabindex="-1">
            <Button
              variant="outline"
              size="sm"
              class="h-6 gap-1 px-2 text-xs"
              :disabled="!node.healthy || !!startBlocked"
              @click="starting = true"
            >
              <Gauge class="size-3" /> Benchmark
            </Button>
          </span>
        </TooltipTrigger>
        <TooltipContent class="max-w-64">{{ startBlocked }}</TooltipContent>
      </Tooltip>
    </div>

    <p v-if="!node.healthy" class="text-xs text-muted-foreground">Not available while unreachable.</p>
    <p v-else-if="cap === undefined && !capError" class="text-xs text-muted-foreground">Loading…</p>
    <LoadError
      v-if="node.healthy"
      :error="capError && `Couldn’t read capacity: ${capError}`"
      :retry="() => store.load(node.id)"
    />
    <template v-if="node.healthy && cap">
      <div v-if="running" class="rounded-md border border-primary/30 bg-primary/5 p-2.5 space-y-2 text-xs">
        <div class="flex items-center gap-2">
          <Loader2 class="size-3.5 shrink-0 animate-spin text-primary" />
          <span class="font-medium truncate">Benchmarking {{ setupLabel(running) }}</span>
          <span class="num text-muted-foreground shrink-0">{{ formatLabel(running.media) }}</span>
          <div class="flex-1" />
          <Button variant="ghost" size="sm" class="h-6 px-2 text-xs" :disabled="cancelling" @click="cancel">
            {{ cancelling ? 'Cancelling…' : 'Cancel' }}
          </Button>
        </div>

        <dl class="grid grid-cols-[auto_1fr] items-baseline gap-x-3 gap-y-1.5">
          <dt class="text-muted-foreground">Result so far</dt>
          <dd>
            <span class="num text-base font-semibold leading-none">{{ running.sustainable_feeds }}</span>
            {{ running.sustainable_feeds === 1 ? 'feed' : 'feeds' }} kept up
            <span class="text-muted-foreground">· searching up to <span class="num">{{ running.max_feeds }}</span></span>
          </dd>

          <template v-if="running.steps.length">
            <dt class="text-muted-foreground">Steps</dt>
            <dd class="flex flex-wrap gap-1">
              <span
                v-for="(step, i) in running.steps"
                :key="i"
                class="num inline-flex items-center gap-0.5 rounded border px-1 leading-4"
                :class="[
                  step.passed ? 'border-success/40 text-success' : 'border-destructive/40 text-destructive',
                  step.quick && 'border-dashed',
                ]"
                :title="stepTitle(step)"
              >
                {{ step.feeds }}<component :is="step.passed ? Check : X" class="size-3" />
              </span>
            </dd>
          </template>

          <dt class="text-muted-foreground">Now</dt>
          <dd>
            <span class="num font-medium">{{ plural(running.feeds_running, 'feed') }}</span>
            <span v-if="reading" class="text-muted-foreground">
              · <span class="num" :class="!reading.passed && 'text-warning'">{{ reading.drop_pct.toFixed(2) }}%</span>
              dropped · CPU <span class="num">{{ Math.round(reading.cpu_pct) }}%</span>
              <span v-if="reading.quick">(quick check)</span>
            </span>
            <span v-else class="text-muted-foreground"> · measuring…</span>
          </dd>

          <dt class="text-muted-foreground">Next</dt>
          <dd class="text-muted-foreground">
            <template v-if="running.next_feeds == null">Done, if this keeps up</template>
            <template v-else>
              <span class="num text-foreground">{{ plural(running.next_feeds, 'feed') }}</span>
              {{ running.next_feeds === running.feeds_running ? 'again, measured in full,' : '' }} if this keeps up
            </template>
          </dd>
        </dl>
      </div>

      <div v-if="active" class="text-xs">
        <div class="flex justify-between gap-2 mb-1">
          <span>
            <template v-if="active > cap.unknown_feeds">
              Using <span class="num">{{ loadPct }}%</span> of benchmarked capacity
            </template>
            <template v-else>No benchmark matches what's running</template>
          </span>
          <span class="text-muted-foreground shrink-0 num">
            {{ activeLabel }}
            <template v-if="cap.unknown_feeds && cap.unknown_feeds < active"> · {{ cap.unknown_feeds }} not benchmarked</template>
          </span>
        </div>
        <div v-if="active > cap.unknown_feeds" class="h-1.5 rounded-full bg-muted overflow-hidden">
          <div class="h-full rounded-full" :class="loadClass" :style="{ width: `${Math.min(loadPct, 100)}%` }" />
        </div>
      </div>

      <ul v-if="profiles.length" class="space-y-1">
        <li v-for="p in profiles.slice(0, SHOWN)" :key="p.run_id">
          <button
            type="button"
            class="w-full text-left rounded-md -mx-1.5 px-1.5 py-1 hover:bg-accent text-xs"
            :title="profileTitle(p)"
            @click="history = { selectId: p.run_id }"
          >
            <span class="flex items-center gap-2">
              <span class="truncate">{{ setupLabel(p) }}</span>
              <span class="num text-muted-foreground shrink-0">{{ formatLabel(p.media) }}</span>
              <span class="flex-1" />
              <span class="num font-medium shrink-0" :class="p.sustainable_feeds === 0 && 'text-destructive'">
                {{ feedsLabel(p) }}
              </span>
            </span>
            <span class="flex gap-2 text-muted-foreground">
              <span class="num truncate">{{ p.encoders.join(', ') }}</span>
              <span class="flex-1" />
              <span v-if="!p.channel" class="num shrink-0">{{ formatRate(p.bytes_per_sec_per_feed) }} per feed</span>
            </span>
          </button>
        </li>
        <li v-if="profiles.length > SHOWN">
          <Button variant="link" size="sm" class="h-5 px-0 text-xs text-muted-foreground" @click="history = {}">
            {{ profiles.length - SHOWN }} more…
          </Button>
        </li>
      </ul>
      <p v-else-if="!running" class="text-xs text-muted-foreground">
        Not benchmarked yet. A benchmark finds how many feeds this node can record with a preset, keep replay
        buffers of, or play out on channels.
      </p>
    </template>
  </section>

  <BenchmarkDialog v-if="starting" :node-id="node.id" @close="starting = false" />
  <BenchmarkRunsDialog v-if="history" :node-id="node.id" :select-id="history.selectId" @close="history = null" />
</template>
