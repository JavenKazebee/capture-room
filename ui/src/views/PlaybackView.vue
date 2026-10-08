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
import type { PlaylistItemDto } from '@/types/generated/PlaylistItemDto'
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
import PlaylistPanel from '@/components/playback/PlaylistPanel.vue'
import { TRANSPORT } from '@/components/playback/transport'

const sources = useSourcesStore()
const nodes = useNodesStore()
const playout = usePlayoutStore()
const route = useRoute()
const router = useRouter()

const SHORTCUTS: [string[], string][] = [
  [['Space'], 'Play / pause'],
  [['N'], 'Take the next item'],
  [['Enter'], 'Cue the selected item'],
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
const playlist = computed(() => (channel.value ? playout.playlists.get(channel.value.key) : undefined))
const items = computed(() => playlist.value?.items ?? [])

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
    playout.loadPlaylist(c.node_id, c.id).catch(() => {})
    if (!playout.media.has(c.node_id)) playout.loadMedia(c.node_id).catch(() => {})
  },
  { immediate: true },
)

// ── Item setup ────────────────────────────────────────────────────────────────

/** The playlist item picked for editing and cueing. */
const selectedId = ref<string | null>(null)
const selected = computed(() => items.value.find((i) => i.id === selectedId.value) ?? null)
/** The library entry picked (only for the library's own highlight). */
const mediaId = ref<string | null>(null)
const loaded = computed(() => status.value?.clip ?? null)
const onAir = computed(() => !!selected.value && status.value?.item_id === selected.value.id)
const nextItem = computed(() => items.value.find((i) => i.id === status.value?.next_id) ?? null)

const inText = ref('')
const outText = ref('')
const end = ref<ClipEnd>('next')

/** Picking an item (or the item changing) shows its setup. */
watch(
  selected,
  (i) => {
    inText.value = i?.in_ms ? formatClock(i.in_ms) : ''
    outText.value = i?.out_ms != null ? formatClock(i.out_ms) : ''
    end.value = i?.end ?? 'next'
  },
  { immediate: true },
)
// Follow the channel's item when it changes (auto-advance, another operator).
watch(
  () => status.value?.item_id,
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
  const d = selected.value?.duration_ms
  if (d != null && inMs.value >= d) return `The in point is past the end (${formatClock(d)}).`
  return null
})

/** The setup differs from the saved item. */
const changed = computed(() => {
  const i = selected.value
  if (!i) return false
  return i.in_ms !== inMs.value || i.out_ms !== outMs.value || i.end !== end.value
})
/** On air, new in and out points only apply once it's cued again. */
const onAirStale = computed(() => {
  const l = loaded.value
  const i = selected.value
  return onAir.value && !!l && !!i && (l.in_ms !== i.in_ms || l.out_ms !== i.out_ms)
})

function setAtPlayhead(which: 'in' | 'out') {
  const p = status.value?.position_ms
  if (p == null || !onAir.value) return
  if (which === 'in') inText.value = formatClock(p)
  else outText.value = formatClock(p)
}

// ── Commands ──────────────────────────────────────────────────────────────────

const busy = ref(false)

async function run(what: string, fn: () => Promise<unknown>) {
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

function applySetup() {
  const c = channel.value
  const i = selected.value
  if (!c || !i || setupError.value || !changed.value) return
  const list = items.value.map((x) => ({
    id: x.id,
    media_id: x.media_id,
    in_ms: x.id === i.id ? inMs.value! : x.in_ms,
    out_ms: x.id === i.id ? outMs.value : x.out_ms,
    end: x.id === i.id ? end.value : x.end,
  }))
  run(`Saving ${i.name}`, () => playout.savePlaylist(c.node_id, c.id, list, playlist.value?.loop_playlist ?? false))
}

function cue(item: PlaylistItemDto | null = selected.value) {
  const c = channel.value
  if (!c || !item) return
  selectedId.value = item.id
  run(`Cueing ${item.name}`, () => playout.cue(c.node_id, c.id, item.id))
}

function transport(action: TransportAction, positionMs?: number) {
  const c = channel.value
  if (!c) return
  const what = action === 'next' ? 'Taking the next item' : action[0].toUpperCase() + action.slice(1)
  run(what, () => playout.transport(c.node_id, c.id, action, positionMs))
}

const playlistPanel = ref<InstanceType<typeof PlaylistPanel> | null>(null)

function addMedia(item: MediaItemDto) {
  mediaId.value = item.id
  playlistPanel.value?.add(item.id)
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
  } else if (key === 'n') {
    if (status.value?.next_id || (!loaded.value && items.value.length)) transport('next')
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
            :next-name="nextItem?.name ?? null"
            :can-next="!!status?.next_id || (!loaded && items.length > 0)"
            @play="transport('play')"
            @pause="transport('pause')"
            @stop="transport('stop')"
            @next="transport('next')"
            @seek="(ms) => transport('seek', ms)"
          />

          <PlaylistPanel
            ref="playlistPanel"
            v-model:selected="selectedId"
            :channel="channel"
            :status="status"
            @cue="cue"
          />

          <!-- Item setup -->
          <section v-if="selected" class="rounded-lg border border-border p-3 flex flex-col gap-3">
            <div class="flex items-center gap-2 min-w-0">
              <span class="text-xs text-muted-foreground shrink-0">Item</span>
              <span class="text-sm font-medium truncate">{{ selected.name }}</span>
              <span v-if="selected.duration_ms != null" class="num text-xs text-muted-foreground shrink-0">
                {{ formatClock(selected.duration_ms) }}
              </span>
            </div>
            <div class="flex flex-wrap items-end gap-3">
              <FormField label="In">
                <div class="flex gap-1">
                  <Input v-model="inText" class="w-24 num h-8" placeholder="0:00.0" @keydown.enter="applySetup" />
                  <Button
                    variant="outline"
                    size="icon"
                    class="size-8"
                    :disabled="!onAir"
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
                  <Input v-model="outText" class="w-24 num h-8" placeholder="End" @keydown.enter="applySetup" />
                  <Button
                    variant="outline"
                    size="icon"
                    class="size-8"
                    :disabled="!onAir"
                    title="Set at the playhead (O)"
                    aria-label="Set the out point at the playhead"
                    @click="setAtPlayhead('out')"
                  >
                    <ArrowDownToLine class="size-3.5" />
                  </Button>
                </div>
              </FormField>
              <FormField label="At the out point">
                <ToggleGroup v-model="end" type="single" variant="segmented">
                  <ToggleGroupItem value="next" class="h-8 px-3 text-xs">Next</ToggleGroupItem>
                  <ToggleGroupItem value="hold" class="h-8 px-3 text-xs">Hold</ToggleGroupItem>
                  <ToggleGroupItem value="black" class="h-8 px-3 text-xs">Black</ToggleGroupItem>
                  <ToggleGroupItem value="loop" class="h-8 px-3 text-xs">Loop</ToggleGroupItem>
                </ToggleGroup>
              </FormField>
              <div class="flex-1" />
              <Button
                size="sm"
                variant="outline"
                class="h-8"
                :disabled="!changed || !!setupError || busy"
                @click="applySetup"
              >
                Apply
              </Button>
              <Button
                size="sm"
                class="h-8 min-w-24"
                :variant="onAir && !onAirStale ? 'outline' : 'default'"
                :disabled="selected.missing || changed || busy"
                :title="changed ? 'Apply the changes first' : 'Load the item, paused at its in point (Enter)'"
                @click="cue()"
              >
                {{ onAir ? 'Re-cue' : 'Cue' }}
              </Button>
            </div>
            <p v-if="setupError" class="text-xs text-destructive">{{ setupError }}</p>
            <p v-else-if="onAirStale" class="text-xs text-muted-foreground">
              It's on air with its old in and out points: cue it again to use the new ones.
            </p>
          </section>
        </div>
      </main>

      <!-- Media -->
      <aside class="w-80 shrink-0 border-l border-border min-h-0">
        <MediaLibrary
          v-if="nodeId"
          v-model:selected="mediaId"
          :node-id="nodeId"
          :loaded-id="loaded?.media_id ?? null"
          @add="addMedia"
        />
      </aside>
    </div>
  </div>
</template>
