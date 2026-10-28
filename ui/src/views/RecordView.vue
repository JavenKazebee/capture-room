<script setup lang="ts">
import { computed, ref } from 'vue'
import { useViewKeys } from '@/composables/useViewKeys'
import { Circle, Eye, FilterX, ListChecks, Plus, Square, X } from '@lucide/vue'
import { registerCommands, type Command } from '@/composables/useCommands'
import { useSourcesStore } from '@/stores/sources'
import { useNodesStore } from '@/stores/nodes'
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
import { useFitTiles } from '@/components/record/useFitTiles'
import SourceFormSheet from '@/components/sources/SourceFormSheet.vue'

const sources = useSourcesStore()
const nodes = useNodesStore()
const desk = useRecordDeskStore()
const { tileSize, tileFit, overlays } = usePreferences()

const gridScroller = ref<HTMLElement>()
// The Add source tile takes a slot too.
const fit = useFitTiles(gridScroller, computed(() => desk.visible.length + 1), { gap: 16, min: 160, max: 960 })
/** The tile width in use: fitted, or the one picked on the slider. */
const tileWidth = computed(() => (tileFit.value ? fit.size.value : tileSize.value))

const adding = ref(false)

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
  [['I'], 'Details for the last feed clicked'],
  [['Esc'], 'Close details, then clear the selection'],
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
  out.push({ id: 'record:add-source', title: 'Add source', group: 'This page', icon: Plus, keywords: ['new', 'create', 'feed'], run: () => (adding.value = true) })
  out.push({ id: 'record:select-all', title: 'Select all visible feeds', group: 'This page', icon: ListChecks, shortcut: shortcut('A'), keywords: ['selection'], run: () => desk.selectAll() })
  if (desk.filtered) {
    out.push({ id: 'record:clear-filters', title: 'Clear filters', group: 'This page', icon: FilterX, keywords: ['reset', 'show all'], run: () => desk.clearFilters() })
  }
  return out
})

useViewKeys((e) => {
  // Esc closes an open info popover first, then clears the selection.
  if (e.key === 'Escape') {
    if (desk.infoKey) desk.infoKey = null
    else desk.clearSelection()
  }
  else if (e.key.toLowerCase() === 'i' && !e.ctrlKey && !e.metaKey && !e.altKey && desk.focusedKey) {
    desk.infoKey = desk.infoKey === desk.focusedKey ? null : desk.focusedKey
  } else if (e.key.toLowerCase() === 'a' && (e.ctrlKey || e.metaKey)) {
    e.preventDefault()
    desk.selectAll()
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
      </template>
    </PageHeader>

    <div
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
        class="grid gap-4"
        :style="{ gridTemplateColumns: `repeat(auto-fill, minmax(min(100%, ${tileWidth}px), ${tileWidth}px))` }"
        @click.self="desk.clearSelection()"
      >
        <FeedTile v-for="source in desk.visible" :key="source.key" :source="source" />
        <button
          type="button"
          class="min-h-24 aspect-video rounded-lg border border-dashed border-border flex flex-col items-center justify-center gap-1.5 text-xs text-muted-foreground transition-colors hover:border-primary hover:text-foreground"
          @click="adding = true"
        >
          <Plus class="size-5" />
          Add source
        </button>
      </div>
    </div>
    <SourceFormSheet v-if="adding" :source="null" @close="adding = false" />
  

  </div>
</template>
