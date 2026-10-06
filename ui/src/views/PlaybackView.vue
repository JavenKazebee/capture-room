<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useEventListener, useStorage } from '@vueuse/core'
import { ArrowDownToLine, Plus } from '@lucide/vue'
import { useSourcesStore } from '@/stores/sources'
import { useNodesStore } from '@/stores/nodes'
import { usePlayoutStore } from '@/stores/playout'
import { notifyError } from '@/lib/notify'
import { formatClock, parseClock } from '@/lib/format'
import type { ClipEnd } from '@/types/generated/ClipEnd'
import type { MediaItemDto } from '@/types/generated/MediaItemDto'
import type { TransportAction } from '@/types/generated/TransportAction'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import PageHeader from '@/components/common/PageHeader.vue'
import StatusDot from '@/components/common/StatusDot.vue'
import FormField from '@/components/FormField.vue'
import ProgramMonitor from '@/components/playback/ProgramMonitor.vue'
import TransportBar from '@/components/playback/TransportBar.vue'
import MediaLibrary from '@/components/playback/MediaLibrary.vue'
import { TRANSPORT } from '@/components/playback/transport'

const sources = useSourcesStore()
const nodes = useNodesStore()
const playout = usePlayoutStore()
const route = useRoute()
const router = useRouter()

const SHORTCUTS: [string[], string][] = [
  [['Space'], 'Play / pause'],
  [['Enter'], 'Cue the selected clip'],
  [['I'], 'Set the in point at the playhead'],
  [['O'], 'Set the out point at the playhead'],
  [['Esc'], 'Stop (back to black)'],
]

// ── Channel ───────────────────────────────────────────────────────────────────

const channelKey = useStorage('cr.playback.channel', '')
const channel = computed(
  () => playout.channels.find((c) => c.key === channelKey.value) ?? playout.channels[0] ?? null,
)
const status = computed(() => (channel.value ? playout.status.get(channel.value.key) : undefined))
const nodeId = computed(() => channel.value?.node_id ?? nodes.self?.node_id ?? '')

// `?channel=<key>` (from Sources) picks the channel.
watch(
  () => route.query.channel,
  (key) => {
    if (typeof key !== 'string') return
    channelKey.value = key
    router.replace({ query: {} })
  },
  { immediate: true },
)

watch(
  channel,
  (c) => {
    if (!c) return
    playout.loadStatus(c.node_id, c.id).catch(() => {})
    if (!playout.media.has(c.node_id)) playout.loadMedia(c.node_id).catch(() => {})
  },
  { immediate: true },
)

// ── Clip setup ────────────────────────────────────────────────────────────────

/** The library entry picked for cueing. */
const selectedId = ref<string | null>(null)
const selected = computed(() => playout.media.get(nodeId.value)?.find((m) => m.id === selectedId.value) ?? null)
const loaded = computed(() => status.value?.clip ?? null)

const inText = ref('')
const outText = ref('')
const end = ref<ClipEnd>('hold')

/** Picking a clip resets its in and out, unless it's the one on air. */
watch(selectedId, () => {
  const l = loaded.value
  if (l && l.media_id === selectedId.value) {
    inText.value = l.in_ms ? formatClock(l.in_ms) : ''
    outText.value = l.out_ms != null ? formatClock(l.out_ms) : ''
    end.value = l.end
  } else {
    inText.value = ''
    outText.value = ''
  }
})
// Follow the channel's clip when it changes (another operator, or a cue).
watch(
  () => loaded.value?.media_id,
  (id) => {
    if (id) selectedId.value = id
  },
  { immediate: true },
)

const inMs = computed(() => (inText.value.trim() ? parseClock(inText.value) : 0))
const outMs = computed(() => (outText.value.trim() ? parseClock(outText.value) : null))
const setupError = computed(() => {
  if (inMs.value === null) return 'The in point should look like 1:23.4 or 83.4.'
  if (outText.value.trim() && outMs.value === null) return 'The out point should look like 1:23.4 or 83.4.'
  if (outMs.value !== null && outMs.value <= inMs.value) return 'The out point must be after the in point.'
  const d = selected.value?.info.duration_ms
  if (d != null && inMs.value >= d) return `The in point is past the end (${formatClock(d)}).`
  return null
})

/** The picked clip, in and out differ from what's on air. */
const changed = computed(() => {
  const l = loaded.value
  if (!l || !selected.value) return false
  return l.media_id !== selected.value.id || l.in_ms !== inMs.value || l.out_ms !== outMs.value || l.end !== end.value
})

function setAtPlayhead(which: 'in' | 'out') {
  const p = status.value?.position_ms
  if (p == null || !loaded.value || loaded.value.media_id !== selectedId.value) return
  if (which === 'in') inText.value = formatClock(p)
  else outText.value = formatClock(p)
}

// ── Commands ──────────────────────────────────────────────────────────────────

const busy = ref(false)

async function run(what: string, fn: () => Promise<void>) {
  if (busy.value) return
  busy.value = true
  try {
    await fn()
  } catch (e) {
    notifyError(`${what} failed`, e, nodeId.value)
  } finally {
    busy.value = false
  }
}

function cue(item: MediaItemDto | null = selected.value) {
  const c = channel.value
  if (!c || !item) return
  // The in and out typed apply to the clip they were typed for; another
  // clip (double-clicked in the list) cues whole.
  const sameSetup = item.id === selectedId.value && !setupError.value
  selectedId.value = item.id
  run(`Cueing ${item.name}`, () =>
    playout.load(c.node_id, c.id, {
      media_id: item.id,
      in_ms: sameSetup ? inMs.value : 0,
      out_ms: sameSetup ? outMs.value : null,
      end: end.value,
    }),
  )
}

function transport(action: TransportAction, positionMs?: number) {
  const c = channel.value
  if (!c) return
  run(action[0].toUpperCase() + action.slice(1), () => playout.transport(c.node_id, c.id, action, positionMs))
}

// ── Keyboard ──────────────────────────────────────────────────────────────────

useEventListener('keydown', (e: KeyboardEvent) => {
  const t = e.target as HTMLElement
  if (t.closest('input, textarea, select, [role="dialog"], [role="menu"]')) return
  if (e.ctrlKey || e.metaKey || e.altKey) return
  const key = e.key.toLowerCase()
  if (key === ' ') {
    e.preventDefault()
    if (status.value?.state === 'playing') transport('pause')
    else if (loaded.value) transport('play')
  } else if (key === 'enter' && !t.closest('[role="listbox"]')) {
    e.preventDefault()
    cue()
  } else if (key === 'escape') {
    if (loaded.value) transport('stop')
  } else if (key === 'i') {
    setAtPlayhead('in')
  } else if (key === 'o') {
    setAtPlayhead('out')
  }
})

onMounted(async () => {
  if (!sources.sources.length) {
    await nodes.load()
    await sources.loadSources()
  }
})
</script>

<template>
  <div class="h-full flex flex-col min-h-0">
    <PageHeader title="Playback" :count="playout.channels.length" :shortcuts="SHORTCUTS">
      <template #actions>
        <Button size="sm" variant="outline" class="h-7 gap-1.5 text-xs" @click="router.push('/setup/sources?add=channel')">
          <Plus class="size-3.5" /> New channel
        </Button>
      </template>
    </PageHeader>

    <div v-if="!playout.channels.length" class="flex-1 grid place-items-center p-8">
      <div class="max-w-sm text-center text-sm text-muted-foreground flex flex-col items-center gap-3">
        <p>
          No playout channels yet. A channel plays clips from a node's media library out over NDI, at a fixed
          format, and shows up as a source you can record.
        </p>
        <Button size="sm" class="gap-1.5" @click="router.push('/setup/sources?add=channel')">
          <Plus class="size-3.5" /> Add a playout channel
        </Button>
      </div>
    </div>

    <div v-else class="flex-1 min-h-0 flex">
      <!-- Channels -->
      <nav class="w-52 shrink-0 border-r border-border overflow-y-auto" aria-label="Channels">
        <button
          v-for="c in playout.channels"
          :key="c.key"
          class="w-full text-left px-3 py-2 border-b border-border/60 flex flex-col gap-0.5"
          :class="c.key === channel?.key ? 'bg-accent' : 'hover:bg-accent/40'"
          :aria-current="c.key === channel?.key || undefined"
          @click="channelKey = c.key"
        >
          <span class="flex items-center gap-1.5 text-sm font-medium min-w-0">
            <StatusDot :status="c.error ? 'error' : TRANSPORT[playout.status.get(c.key)?.state ?? 'idle'].dot" />
            <span class="truncate">{{ c.display_name }}</span>
          </span>
          <span class="text-[11px] text-muted-foreground truncate">
            {{ TRANSPORT[playout.status.get(c.key)?.state ?? 'idle'].label }}
            <template v-if="playout.status.get(c.key)?.clip"> · {{ playout.status.get(c.key)!.clip!.name }}</template>
          </span>
          <span v-if="nodes.nodes.length > 1" class="text-[10px] uppercase tracking-wide text-muted-foreground">
            {{ nodes.nameOf(c.node_id) }}
          </span>
        </button>
      </nav>

      <!-- Program, transport, clip -->
      <main v-if="channel" class="flex-1 min-w-0 overflow-y-auto p-4">
        <div class="max-w-4xl mx-auto flex flex-col gap-4">
          <ProgramMonitor :source="channel" :status="status" />
          <div
            v-if="status?.error"
            class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive"
          >
            {{ status.error }}
          </div>

          <TransportBar
            :status="status"
            :busy="busy"
            @play="transport('play')"
            @pause="transport('pause')"
            @stop="transport('stop')"
            @seek="(ms) => transport('seek', ms)"
          />

          <!-- Clip setup -->
          <section class="rounded-lg border border-border p-3 flex flex-col gap-3">
            <div class="flex items-center gap-2 min-w-0">
              <span class="text-xs text-muted-foreground shrink-0">Clip</span>
              <span v-if="selected" class="text-sm font-medium truncate" :title="selected.path">{{ selected.name }}</span>
              <span v-else class="text-sm text-muted-foreground">Pick a clip in the media list.</span>
              <span v-if="selected?.info.duration_ms != null" class="num text-xs text-muted-foreground shrink-0">
                {{ formatClock(selected.info.duration_ms) }}
              </span>
            </div>
            <div class="flex flex-wrap items-end gap-3">
              <FormField label="In">
                <div class="flex gap-1">
                  <Input v-model="inText" class="w-24 num h-8" placeholder="0:00.0" :disabled="!selected" />
                  <Button
                    variant="outline"
                    size="icon"
                    class="size-8"
                    :disabled="!loaded || loaded.media_id !== selectedId"
                    title="Set at the playhead (I)"
                    aria-label="Set the in point at the playhead"
                    @click="setAtPlayhead('in')"
                  >
                    <ArrowDownToLine class="size-3.5" />
                  </Button>
                </div>
              </FormField>
              <FormField label="Out">
                <div class="flex gap-1">
                  <Input v-model="outText" class="w-24 num h-8" placeholder="End" :disabled="!selected" />
                  <Button
                    variant="outline"
                    size="icon"
                    class="size-8"
                    :disabled="!loaded || loaded.media_id !== selectedId"
                    title="Set at the playhead (O)"
                    aria-label="Set the out point at the playhead"
                    @click="setAtPlayhead('out')"
                  >
                    <ArrowDownToLine class="size-3.5" />
                  </Button>
                </div>
              </FormField>
              <FormField label="At the out point">
                <ToggleGroup v-model="end" type="single" variant="segmented" :disabled="!selected">
                  <ToggleGroupItem value="hold" class="h-8 px-3 text-xs">Hold</ToggleGroupItem>
                  <ToggleGroupItem value="black" class="h-8 px-3 text-xs">Black</ToggleGroupItem>
                  <ToggleGroupItem value="loop" class="h-8 px-3 text-xs">Loop</ToggleGroupItem>
                </ToggleGroup>
              </FormField>
              <div class="flex-1" />
              <Button
                size="sm"
                class="h-8 min-w-24"
                :variant="loaded && !changed ? 'outline' : 'default'"
                :disabled="!selected || selected.missing || !!setupError || busy"
                title="Load the clip, paused at its in point (Enter)"
                @click="cue()"
              >
                {{ loaded && changed ? 'Re-cue' : 'Cue' }}
              </Button>
            </div>
            <p v-if="setupError" class="text-xs text-destructive">{{ setupError }}</p>
            <p v-else-if="loaded && changed" class="text-xs text-muted-foreground">
              Cue again to put these changes on air.
            </p>
          </section>
        </div>
      </main>

      <!-- Media -->
      <aside class="w-80 shrink-0 border-l border-border min-h-0">
        <MediaLibrary
          v-if="nodeId"
          v-model:selected="selectedId"
          :node-id="nodeId"
          :loaded-id="loaded?.media_id ?? null"
          @cue="cue"
        />
      </aside>
    </div>
  </div>
</template>
