<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useViewKeys } from '@/composables/useViewKeys'
import { registerCommands } from '@/composables/useCommands'
import { Plus, Settings2 } from '@lucide/vue'
import { useSourcesStore } from '@/stores/sources'
import { useNodesStore } from '@/stores/nodes'
import { usePlayoutStore } from '@/stores/playout'
import { notifyError } from '@/lib/notify'
import type { ClipEnd } from '@/types/generated/ClipEnd'
import type { MediaItemDto } from '@/types/generated/MediaItemDto'
import type { PlaylistItemDto } from '@/types/generated/PlaylistItemDto'
import type { TransportAction } from '@/types/generated/TransportAction'
import { Button } from '@/components/ui/button'
import PageHeader from '@/components/common/PageHeader.vue'
import StatusDot from '@/components/common/StatusDot.vue'
import ProgramMonitor from '@/components/playback/ProgramMonitor.vue'
import TransportBar from '@/components/playback/TransportBar.vue'
import MediaLibrary from '@/components/playback/MediaLibrary.vue'
import PlaylistPanel from '@/components/playback/PlaylistPanel.vue'
import ChannelSheet from '@/components/playback/ChannelSheet.vue'
import ItemSetup from '@/components/playback/ItemSetup.vue'
import { useChannelChoice } from '@/components/playback/useChannelChoice'
import { TRANSPORT } from '@/components/playback/transport'

const sources = useSourcesStore()
const nodes = useNodesStore()
const playout = usePlayoutStore()
const route = useRoute()
const router = useRouter()

const SHORTCUTS: [string[], string][] = [
  [['Space'], 'Play / pause'],
  [['N'], 'Play the next item'],
  [['Enter'], 'Play the selected item'],
  [['I'], 'Set the in point at the playhead'],
  [['O'], 'Set the out point at the playhead'],
  [['Esc'], 'Stop (back to black)'],
]

const { channelKey, channel } = useChannelChoice('cr.playback.channel')
const status = computed(() => (channel.value ? playout.status.get(channel.value.key) : undefined))
const nodeId = computed(() => channel.value?.node_id ?? nodes.self?.node_id ?? '')
const playlist = computed(() => (channel.value ? playout.playlists.get(channel.value.key) : undefined))
const items = computed(() => playlist.value?.items ?? [])

/** The channel sheet: `'new'`, the channel being edited, or closed. */
const sheet = ref<'new' | { nodeId: string; id: string } | null>(null)

registerCommands(() => [
  { id: 'playback:new-channel', title: 'New playout channel', group: 'This page', icon: Plus, keywords: ['add', 'create', 'output'], run: () => (sheet.value = 'new') },
  ...(channel.value
    ? [{ id: 'playback:channel-settings', title: `${channel.value.display_name} settings`, group: 'This page' as const, icon: Settings2, keywords: ['channel', 'edit', 'configure'], run: () => editChannel(channel.value!.key) }]
    : []),
])

function editChannel(key: string) {
  const c = playout.channels.find((c) => c.key === key)
  if (c) sheet.value = { nodeId: c.node_id, id: c.id }
}

// From Sources: `?channel=<key>` picks the channel, with `&edit` opening its
// settings; `?add=channel` opens a new one.
watch(
  () => [route.query.channel, route.query.edit, route.query.add],
  async ([key, edit, add]) => {
    if (typeof key === 'string') {
      channelKey.value = key
      if (edit !== undefined) {
        await sources.ensureLoaded()
        editChannel(key)
      }
    } else if (add === 'channel') {
      sheet.value = 'new'
    } else {
      return
    }
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

/** The playlist item picked for editing and cueing. */
const selectedId = ref<string | null>(null)
const selected = computed(() => items.value.find((i) => i.id === selectedId.value) ?? null)
/** The library entry picked (only for the library's own highlight). */
const mediaId = ref<string | null>(null)
const loaded = computed(() => status.value?.clip ?? null)
const nextItem = computed(() => items.value.find((i) => i.id === status.value?.next_id) ?? null)
const itemSetup = ref<InstanceType<typeof ItemSetup> | null>(null)

// Follow the channel's item when it changes (auto-advance, another operator).
watch(
  () => status.value?.item_id,
  (id) => {
    if (id) selectedId.value = id
  },
  { immediate: true },
)

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

/** Save the selected item's new in, out and end into the playlist. */
function applySetup(setup: { in_ms: number; out_ms: number | null; end: ClipEnd }) {
  const c = channel.value
  const i = selected.value
  if (!c || !i) return
  const list = items.value.map((x) => ({
    id: x.id,
    media_id: x.media_id,
    ...(x.id === i.id ? setup : { in_ms: x.in_ms, out_ms: x.out_ms, end: x.end }),
  }))
  run(`Saving ${i.name}`, () => playout.savePlaylist(c.node_id, c.id, list, playlist.value?.loop_playlist ?? false))
}

function play(item: PlaylistItemDto | null = selected.value) {
  const c = channel.value
  if (!c || !item || item.missing) return
  selectedId.value = item.id
  run(`Playing ${item.name}`, () => playout.playItem(c.node_id, c.id, item.id))
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
  const what = action === 'next' ? 'Playing the next item' : action[0].toUpperCase() + action.slice(1)
  run(what, () => playout.transport(c.node_id, c.id, action, { position_ms: positionMs ?? null }))
}

const playlistPanel = ref<InstanceType<typeof PlaylistPanel> | null>(null)

function addMedia(item: MediaItemDto) {
  mediaId.value = item.id
  playlistPanel.value?.add(item.id)
}

useViewKeys((e) => {
  const t = e.target as HTMLElement
  const key = e.key.toLowerCase()
  if (key === ' ') {
    e.preventDefault()
    if (status.value?.state === 'playing') transport('pause')
    else if (loaded.value || status.value?.next_id) transport('play')
  } else if (key === 'enter' && !t.closest('[role="listbox"]')) {
    e.preventDefault()
    play()
  } else if (key === 'n') {
    if (status.value?.next_id) transport('next')
  } else if (key === 'escape') {
    if (loaded.value) transport('stop')
  } else if (key === 'i') {
    itemSetup.value?.setAtPlayhead('in')
  } else if (key === 'o') {
    itemSetup.value?.setAtPlayhead('out')
  }
})

onMounted(() => sources.ensureLoaded())
</script>

<template>
  <div class="h-full flex flex-col min-h-0">
    <PageHeader title="Playback" :count="playout.channels.length" :shortcuts="SHORTCUTS">
      <template #actions>
        <Button v-if="channel" size="sm" variant="outline" class="h-7 gap-1.5 text-xs" @click="editChannel(channel.key)">
          <Settings2 class="size-3.5" /> Channel settings
        </Button>
        <Button size="sm" variant="outline" class="h-7 gap-1.5 text-xs" @click="sheet = 'new'">
          <Plus class="size-3.5" /> New channel
        </Button>
      </template>
    </PageHeader>

    <div v-if="!playout.channels.length" class="flex-1 grid place-items-center p-8">
      <div class="max-w-sm text-center text-sm text-muted-foreground flex flex-col items-center gap-3">
        <p>
          No playout channels yet. A channel plays clips from a node's media library out at a fixed format (over
          NDI, SRT, RTSP or to a screen) and shows up as a source you can record.
        </p>
        <Button size="sm" class="gap-1.5" @click="sheet = 'new'">
          <Plus class="size-3.5" /> Add a playout channel
        </Button>
      </div>
    </div>

    <div v-else class="flex-1 min-h-0 flex">
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
            :can-next="!!status?.next_id"
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
            @play="play"
          />

          <ItemSetup
            v-if="selected"
            ref="itemSetup"
            :item="selected"
            :status="status"
            :busy="busy"
            @apply="applySetup"
            @cue="cue()"
            @play="play()"
          />
        </div>
      </main>

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

  <ChannelSheet
    v-if="sheet"
    :editing="sheet === 'new' ? null : sheet"
    @close="sheet = null"
    @saved="(key) => (channelKey = key)"
  />
</template>
