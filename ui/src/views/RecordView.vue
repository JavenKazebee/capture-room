<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import { useViewKeys } from '@/composables/useViewKeys'
import { Circle, Eye, FilterX, Flag, ListChecks, PanelRight, Plus, Square, Tag, X } from '@lucide/vue'
import { useSessionStorage, useStorage } from '@vueuse/core'
import { registerCommands, type Command } from '@/composables/useCommands'
import { useSourcesStore } from '@/stores/sources'
import { useNodesStore } from '@/stores/nodes'
import { useLayoutStore } from '@/stores/layout'
import { useRecordDeskStore, type StateFilter } from '@/stores/recordDesk'
import { usePreferences, type TileOverlays } from '@/composables/usePreferences'
import { wsStatus } from '@/composables/useWebSocket'
import { shortcut } from '@/lib/keys'
import { Button } from '@/components/ui/button'
import { Slider } from '@/components/ui/slider'
import { Switch } from '@/components/ui/switch'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import OptionSelect from '@/components/OptionSelect.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import ToolbarField from '@/components/common/ToolbarField.vue'
import ToolbarSearch from '@/components/common/ToolbarSearch.vue'
import FeedTile from '@/components/record/FeedTile.vue'
import ExpandedFeed from '@/components/record/ExpandedFeed.vue'
import SessionRecordings from '@/components/record/SessionRecordings.vue'
import MarkerNoteDialog from '@/components/record/MarkerNoteDialog.vue'
import { useRecordingsStore } from '@/stores/recordings'
import { useFitTiles } from '@/components/record/useFitTiles'
import { gridColumns, gridNavTarget } from '@/components/record/gridNav'
import SourceFormSheet from '@/components/sources/SourceFormSheet.vue'

const sources = useSourcesStore()
const recordings = useRecordingsStore()
const nodes = useNodesStore()
const desk = useRecordDeskStore()
const layout = useLayoutStore()
const { tileSize, tileFit, overlays } = usePreferences()

const gridScroller = ref<HTMLElement>()
const GAP = 16
// The Add source tile takes a slot too.
const fit = useFitTiles(gridScroller, computed(() => desk.visible.length + 1), { gap: GAP, min: 160, max: 960 })
/** The tile width in use: fitted, or the one picked on the slider. */
const tileWidth = computed(() => (tileFit.value ? fit.size.value : tileSize.value))

/** Columns the grid lays out right now; it changes with the slider, Fit and the window. */
const columns = computed(() => gridColumns(fit.width.value, tileWidth.value, GAP))

const gridEl = ref<HTMLElement>()
/** The focused tile when it's shown, else the first: where Tab lands and arrows start. */
const tabStopKey = computed(() =>
  desk.visible.some((s) => s.key === desk.focusedKey) ? desk.focusedKey : (desk.visible[0]?.key ?? null),
)

function focusTile(key: string) {
  desk.focusedKey = key
  gridEl.value?.querySelector<HTMLElement>(`[data-key="${CSS.escape(key)}"]`)?.focus()
}

/** Arrow keys move the focused tile; returns whether the key was one. */
function moveFocus(e: KeyboardEvent, fromKey: string | null) {
  const keys = desk.visible.map((s) => s.key)
  const i = gridNavTarget(e.key, fromKey ? keys.indexOf(fromKey) : -1, keys.length, columns.value)
  if (i == null) return false
  e.preventDefault()
  focusTile(keys[i]!)
  return true
}

/** Move a feed to `target`'s place, keeping it focused if it was. */
async function moveFeed(key: string, target: string) {
  const moving = layout.move(key, target, desk.ordered.map((s) => s.key))
  await nextTick()
  if (desk.focusedKey === key) focusTile(key)
  await moving
}

/** Alt+arrow on a tile moves it the way plain arrows move focus; returns whether the key was one. */
function moveTile(e: KeyboardEvent, key: string) {
  const keys = desk.visible.map((s) => s.key)
  const i = gridNavTarget(e.key, keys.indexOf(key), keys.length, columns.value)
  if (i == null) return false
  e.preventDefault()
  desk.focusedKey = key
  if (keys[i] !== key) moveFeed(key, keys[i]!)
  return true
}

/** Keys on a tile, or on one of its plain buttons (arrows only, so Space/Enter still press them). */
function onGridKeyDown(e: KeyboardEvent) {
  const target = e.target as HTMLElement
  const tile = target.closest<HTMLElement>('[data-feed-tile]')
  if (!tile) return
  const onTile = target === tile
  if (e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey && onTile && tile.dataset.key) {
    moveTile(e, tile.dataset.key)
    return
  }
  if (e.ctrlKey || e.metaKey || e.altKey || e.shiftKey) return
  if (!onTile && !(target.tagName === 'BUTTON' && !target.hasAttribute('role'))) return
  if (moveFocus(e, tile.dataset.key ?? null) || !onTile) return
  if (e.key === ' ' || e.key === 'Enter') {
    e.preventDefault()
    // Holding the key would start then stop.
    if (e.repeat) return
    const source = desk.visible.find((s) => s.key === tile.dataset.key)
    if (source) desk.toggle(source)
  }
}

/** The feed expanded to fill the view, by key. The grid stays mounted underneath, hidden. */
const expandedKey = ref<string | null>(null)
const expanded = computed(() => desk.visible.find((s) => s.key === expandedKey.value) ?? null)
const expandedView = ref<InstanceType<typeof ExpandedFeed>>()

function expand(key: string) {
  desk.infoKey = null
  desk.focusedKey = key
  expandedKey.value = key
}

// Opening focuses the expanded feed, so Space records it; going back focuses its tile again.
// A feed that's filtered out or removed while expanded drops back to the grid the same way.
watch(
  () => expanded.value?.key ?? null,
  async (now, prev) => {
    await nextTick()
    if (now) expandedView.value?.focus()
    else if (prev) {
      expandedKey.value = null
      if (desk.visible.some((s) => s.key === prev)) focusTile(prev)
    }
  },
)

/** Drag to reorder: the tile being dragged, and the one it would take the place of. */
const dragKey = ref<string | null>(null)
const dropKey = ref<string | null>(null)

const tileKeyOf = (e: DragEvent) => (e.target as HTMLElement).closest<HTMLElement>('[data-feed-tile]')?.dataset.key ?? null

function dropSideOf(key: string): 'before' | 'after' | null {
  if (!dragKey.value || key !== dropKey.value) return null
  const keys = desk.visible.map((s) => s.key)
  return keys.indexOf(dragKey.value) > keys.indexOf(key) ? 'before' : 'after'
}

function onDragStart(e: DragEvent) {
  const key = tileKeyOf(e)
  if (!key || !e.dataTransfer) return
  dragKey.value = key
  e.dataTransfer.effectAllowed = 'move'
  // Firefox starts no drag without data.
  e.dataTransfer.setData('text/plain', key)
}

function onDragOver(e: DragEvent) {
  if (!dragKey.value) return
  const key = tileKeyOf(e)
  if (!key) return
  e.preventDefault()
  if (e.dataTransfer) e.dataTransfer.dropEffect = 'move'
  dropKey.value = key === dragKey.value ? null : key
}

function onDrop(e: DragEvent) {
  if (!dragKey.value) return
  e.preventDefault()
  if (dropKey.value) moveFeed(dragKey.value, dropKey.value)
  onDragEnd()
}

function onDragEnd() {
  dragKey.value = dropKey.value = null
}

const tagOptions = computed(() => [
  { value: 'all', label: 'All tags' },
  ...layout.allTags.map((t) => ({ value: t, label: t })),
])

const adding = ref(false)

/** The pane of this session's recordings: closed until opened, then remembered. */
const paneOpen = useStorage('cr.record.sessionPane', false)
/** When this tab's session began: its first visit to Record, or the pane's last Clear. */
const sessionSince = useSessionStorage('cr.record.since', new Date().toISOString())

const nodeOptions = computed(() => [
  { value: 'all', label: 'All nodes' },
  ...nodes.nodes.map((n) => ({ value: n.id, label: nodes.nameOf(n.id) })),
])
const typeOptions = computed(() => [
  { value: 'all', label: 'All types' },
  ...[...new Set(sources.sources.map((s) => s.source_type))].map((t) => ({ value: t, label: t.toUpperCase() })),
])

const OVERLAYS: { key: keyof TileOverlays; label: string }[] = [
  { key: 'timecode', label: 'Timecode' },
  { key: 'meters', label: 'Audio meters' },
  { key: 'format', label: 'Format (resolution/fps)' },
  { key: 'node', label: 'Node name' },
]

// Dragging the slider takes over from Fit, starting from the fitted size.
const tileSizeModel = computed({
  get: () => [Math.min(Math.max(tileWidth.value, 200), 640)],
  set: (v: number[] | undefined) => {
    if (!v?.[0]) return
    tileFit.value = false
    tileSize.value = v[0]
  },
})

const SHORTCUTS: [string[], string][] = [
  [[shortcut('Click')], 'Add or remove a feed from the selection'],
  [['Shift', 'Click'], 'Select a range'],
  [[shortcut('A')], 'Select all visible feeds'],
  [['←', '→', '↑', '↓'], 'Move between feeds'],
  [['Space'], 'Record or stop the focused feed'],
  [['M'], 'Mark every recording here, for the edit'],
  [['Shift', 'M'], 'Mark only the focused feed'],
  [['Alt', '←', '→', '↑', '↓'], 'Move the focused feed (or drag it)'],
  [['I'], 'Details for the focused feed'],
  [['T'], 'Tags for the focused feed, or the selection'],
  [['F'], 'Expand the focused feed, or go back'],
  [['Double-click'], 'Expand a feed'],
  [['Esc'], 'Go back, close details, then clear the selection'],
]
// Record/Stop act on the scope: the selection, or every visible feed when nothing is selected.
const scopeLive = computed(() => desk.scope.filter((s) => desk.isLive(s)))
const scopeIdle = computed(() => desk.scope.filter((s) => !desk.isLive(s)))
const allScope = computed(() => desk.selected.size === 0)

registerCommands(() => {
  const scope = allScope.value ? 'all visible feeds' : 'the selected feeds'
  const out: Command[] = []
  if (scopeIdle.value.length) {
    out.push({ id: 'record:scope-start', title: `Record ${scope}`, detail: `${scopeIdle.value.length} idle`, group: 'This page', icon: Circle, iconClass: 'text-tally!', keywords: ['start', 'rec', 'bulk'], run: () => desk.bulk('start', scopeIdle.value) })
  }
  if (scopeLive.value.length) {
    out.push({ id: 'record:scope-stop', title: `Stop ${scope}`, detail: `${scopeLive.value.length} recording`, group: 'This page', icon: Square, iconClass: 'text-tally!', keywords: ['end', 'bulk'], run: () => desk.bulk('stop', scopeLive.value) })
  }
  if (recordings.activeSessions.length) {
    out.push({ id: 'record:mark', title: 'Mark every recording', detail: `${recordings.activeSessions.length} recording`, group: 'This page', icon: Flag, shortcut: 'M', keywords: ['marker', 'edit point', 'flag'], run: () => markAll(performance.now()) })
  }
  out.push({ id: 'record:add-source', title: 'Add source', group: 'This page', icon: Plus, keywords: ['new', 'create', 'feed'], run: () => (adding.value = true) })
  out.push({ id: 'record:session-pane', title: paneOpen.value ? "Hide this session's recordings" : "Show this session's recordings", group: 'This page', icon: PanelRight, keywords: ['recordings', 'takes', 'pane', 'sidebar'], run: () => (paneOpen.value = !paneOpen.value) })
  out.push({ id: 'record:select-all', title: 'Select all visible feeds', group: 'This page', icon: ListChecks, shortcut: shortcut('A'), keywords: ['selection'], run: () => desk.selectAll() })
  for (const tag of layout.allTags) {
    out.push({ id: `record:select-tag:${tag}`, title: `Select feeds tagged ${tag}`, group: 'This page', icon: Tag, keywords: ['tag', 'group', 'selection'], run: () => desk.selectTag(tag) })
  }
  if (tabStopKey.value) {
    const many = desk.selected.size > 1 && desk.selected.has(tabStopKey.value)
    out.push({ id: 'record:tags', title: many ? 'Tag the selected feeds' : 'Tag the focused feed', group: 'This page', icon: Tag, shortcut: 'T', keywords: ['tag', 'group', 'label'], run: () => openTags(tabStopKey.value!) })
  }
  if (desk.filtered) {
    out.push({ id: 'record:clear-filters', title: 'Clear filters', group: 'This page', icon: FilterX, keywords: ['reset', 'show all'], run: () => desk.clearFilters() })
  }
  return out
})

/** Every feed recording here, on any node, filtered out or not. */
function markAll(pressedAt: number) {
  void desk.mark(sources.sources.filter((s) => desk.isLive(s)), pressedAt)
}

function markFocused(pressedAt: number) {
  const key = expandedKey.value ?? tabStopKey.value
  const source = sources.sources.find((s) => s.key === key)
  if (source) void desk.mark([source], pressedAt)
}

function openTags(key: string) {
  desk.infoKey = null
  desk.focusedKey = key
  desk.tagsKey = key
}

useViewKeys((e) => {
  const plain = !e.ctrlKey && !e.metaKey && !e.altKey
  // Esc leaves the expanded feed first, then closes an open info popover, then clears the selection.
  if (e.key === 'Escape') {
    if (expandedKey.value) expandedKey.value = null
    else if (desk.infoKey) desk.infoKey = null
    else desk.clearSelection()
  } else if (e.key.toLowerCase() === 'f' && plain && !e.shiftKey) {
    if (expandedKey.value) expandedKey.value = null
    else if (tabStopKey.value) expand(tabStopKey.value)
  } else if (e.key.toLowerCase() === 't' && plain && !e.shiftKey && tabStopKey.value && !expanded.value) {
    // The popover focuses its field during this keydown: the key would be typed into it.
    e.preventDefault()
    openTags(tabStopKey.value)
  } else if (e.key.toLowerCase() === 'i' && plain && desk.focusedKey && !expanded.value) {
    desk.infoKey = desk.infoKey === desk.focusedKey ? null : desk.focusedKey
  } else if (e.key.toLowerCase() === 'm' && plain) {
    // Timed at the keydown: the mark is where the operator pressed, however
    // long the request takes. Holding the key would mark again and again.
    if (!e.repeat) (e.shiftKey ? markFocused : markAll)(e.timeStamp)
  } else if (e.key.toLowerCase() === 'a' && (e.ctrlKey || e.metaKey)) {
    e.preventDefault()
    desk.selectAll()
  } else if (e.target === document.body && !expanded.value && !e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey) {
    // Nothing has focus (page load, a click on the background): arrows pick up at the focused tile.
    if (e.key.startsWith('Arrow') && tabStopKey.value) {
      e.preventDefault()
      focusTile(tabStopKey.value)
    }
  }
}, { alsoIgnore: '[role="listbox"]', modifiers: true })
</script>

<template>
  <div class="h-full flex flex-col min-h-0">
    <PageHeader title="Record" :count="sources.sources.length" :shortcuts="SHORTCUTS">
      <template #actions>
        <!-- Fixed width and only labels change, so the toolbar never shifts with the selection. -->
        <div class="flex items-center gap-2">
          <span class="w-24 flex items-center gap-1 text-xs tabular-nums">
            <template v-if="allScope">
              <span class="text-muted-foreground">All {{ desk.visible.length }} feeds</span>
            </template>
            <template v-else>
              <span class="font-medium text-primary">{{ desk.selected.size }} selected</span>
              <button
                class="size-5 grid place-items-center rounded text-muted-foreground hover:text-foreground hover:bg-accent"
                aria-label="Clear selection"
                title="Clear selection (Esc)"
                @click="desk.clearSelection()"
              >
                <X class="size-3" />
              </button>
            </template>
          </span>
          <OptionSelect
            :model-value="desk.scopePresetId"
            :options="desk.scopePresetOptions"
            class="w-44 text-xs"
            :title="allScope ? 'Preset for every visible feed' : 'Preset for the selected feeds'"
            @update:model-value="desk.setScopePreset"
          />
          <Button variant="outline" size="sm" class="h-7 min-w-32 gap-1.5 text-xs tabular-nums" :disabled="!scopeIdle.length" @click="desk.bulk('start', scopeIdle)">
            <span class="size-2 rounded-full bg-tally" /> Record{{ allScope ? ' all' : '' }} ({{ scopeIdle.length }})
          </Button>
          <Button variant="outline" size="sm" class="h-7 min-w-28 gap-1.5 text-xs tabular-nums" :disabled="!scopeLive.length" @click="desk.bulk('stop', scopeLive)">
            <span class="size-2 rounded-[1px] bg-tally" /> Stop{{ allScope ? ' all' : '' }} ({{ scopeLive.length }})
          </Button>
        </div>
      </template>

      <ToolbarSearch v-model="desk.query" />
      <ToggleGroup
        :model-value="desk.stateFilter"
        type="single"
        variant="segmented"
        @update:model-value="(v) => v && (desk.stateFilter = v as StateFilter)"
      >
        <ToggleGroupItem value="all" class="px-2.5">All</ToggleGroupItem>
        <ToggleGroupItem value="live" class="px-2.5">Live</ToggleGroupItem>
        <ToggleGroupItem value="idle" class="px-2.5">Idle</ToggleGroupItem>
      </ToggleGroup>
      <OptionSelect v-if="nodes.nodes.length > 1" v-model="desk.nodeFilter" :options="nodeOptions" class="h-7 w-32 text-xs" />
      <OptionSelect v-if="typeOptions.length > 2" v-model="desk.typeFilter" :options="typeOptions" class="h-7 w-28 text-xs" />
      <OptionSelect v-if="layout.allTags.length" v-model="desk.tagFilter" :options="tagOptions" class="h-7 w-32 text-xs" />

      <template #view>
        <DropdownMenu>
          <DropdownMenuTrigger as-child>
            <Button variant="outline" size="sm" class="h-7 gap-1.5 text-xs"><Eye class="size-3.5" /> Overlays</Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" class="w-52">
            <DropdownMenuLabel>Show on tiles</DropdownMenuLabel>
            <DropdownMenuCheckboxItem
              v-for="o in OVERLAYS"
              :key="o.key"
              :model-value="overlays[o.key]"
              @update:model-value="(v) => (overlays = { ...overlays, [o.key]: v })"
              @select.prevent
            >
              {{ o.label }}
            </DropdownMenuCheckboxItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <ToolbarField label="Tile size" title="Tile size (turns Fit off)">
          <Slider v-model="tileSizeModel" :min="200" :max="640" :step="20" class="w-24" :class="tileFit && 'opacity-50'" />
        </ToolbarField>
        <ToolbarField label="Fit" title="Size tiles so every feed fits without scrolling">
          <Switch v-model="tileFit" />
        </ToolbarField>
        <Button
          variant="outline"
          size="sm"
          class="h-7 gap-1.5 text-xs"
          :class="paneOpen && 'bg-accent'"
          :aria-pressed="paneOpen"
          title="Recordings made this session"
          @click="paneOpen = !paneOpen"
        >
          <PanelRight class="size-3.5" /> Recordings
        </Button>
      </template>
    </PageHeader>

    <!-- The pane takes its width from the grid, whose fitting measures what's left. -->
    <div class="flex-1 min-h-0 flex">
      <div class="flex-1 min-w-0 flex flex-col">
        <div v-if="expanded" class="flex-1 min-h-0 p-4">
          <ExpandedFeed ref="expandedView" :source="expanded" @close="expandedKey = null" />
        </div>
        <div
          v-show="!expanded"
          ref="gridScroller"
          class="flex-1 min-h-0 overflow-y-auto p-4"
          :class="wsStatus !== 'connected' && 'opacity-60'"
          @click.self="desk.clearSelection()"
        >
          <div v-if="sources.sources.length === 0" class="flex flex-col items-center gap-3 text-center text-muted-foreground py-24">
            <p class="text-sm font-medium text-foreground">No feeds yet</p>
            <Button size="sm" class="gap-1.5" @click="adding = true"><Plus class="size-3.5" /> Add source</Button>
          </div>
          <div v-else-if="desk.visible.length === 0" class="text-center text-muted-foreground py-24 text-xs">
            No feeds match the filters.
            <button class="text-primary hover:underline ml-1" @click="desk.clearFilters()">Clear filters</button>
          </div>
          <div
            v-else
            ref="gridEl"
            class="grid gap-4"
            :style="{ gridTemplateColumns: `repeat(auto-fill, minmax(min(100%, ${tileWidth}px), ${tileWidth}px))` }"
            @click.self="desk.clearSelection()"
            @keydown="onGridKeyDown"
            @dragstart="onDragStart"
            @dragover="onDragOver"
            @drop="onDrop"
            @dragend="onDragEnd"
          >
            <FeedTile
              v-for="source in desk.visible"
              :key="source.key"
              :source="source"
              :tab-stop="source.key === tabStopKey"
              :dragging="source.key === dragKey"
              :drop-side="dropSideOf(source.key)"
              @expand="expand(source.key)"
            />
            <!-- A tile's picture plus its measured caption, so it matches the feed tiles beside it. -->
            <button
              type="button"
              class="relative self-start rounded-lg border border-dashed border-border flex flex-col text-xs text-muted-foreground transition-colors hover:border-primary hover:text-foreground"
              @click="adding = true"
            >
              <span class="aspect-video" />
              <span :style="{ height: `${fit.caption.value}px` }" />
              <span class="absolute inset-0 flex flex-col items-center justify-center gap-1.5">
                <Plus class="size-5" />
                Add source
              </span>
            </button>
          </div>
        </div>
      </div>
      <SessionRecordings v-if="paneOpen" v-model:since="sessionSince" @close="paneOpen = false" />
    </div>
    <SourceFormSheet v-if="adding" :source="null" @close="adding = false" />
    <MarkerNoteDialog />
  

  </div>
</template>
