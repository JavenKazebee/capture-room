<script setup lang="ts">
import { computed, ref } from 'vue'
import { useEventListener } from '@vueuse/core'
import { Eye, X } from '@lucide/vue'
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

const sources = useSourcesStore()
const nodes = useNodesStore()
const desk = useRecordDeskStore()
const { tileSize, tileFit, overlays } = usePreferences()

const gridScroller = ref<HTMLElement>()
const fit = useFitTiles(gridScroller, computed(() => desk.visible.length), { gap: 16, min: 160, max: 960 })
/** The tile width in use: fitted, or the one picked on the slider. */
const tileWidth = computed(() => (tileFit.value ? fit.size.value : tileSize.value))

// ── Toolbar ───────────────────────────────────────────────────────────────────

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

// ── Keyboard ──────────────────────────────────────────────────────────────────

useEventListener('keydown', (e: KeyboardEvent) => {
  const t = e.target as HTMLElement
  if (t.closest('input, textarea, select, [role="dialog"], [role="listbox"], [role="menu"]')) return
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
})
</script>

<template>
  <div class="h-full flex flex-col min-h-0">
    <PageHeader title="Record" :count="sources.sources.length" :shortcuts="SHORTCUTS">
      <template #actions>
        <!--
          Transport: one fixed-width group acting on the selection, or on every
          visible feed when nothing is selected. Only labels change, so the
          toolbar never shifts when the selection does.
        -->
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
      <div v-if="sources.sources.length === 0" class="text-center text-muted-foreground py-24">
        <p class="text-sm font-medium text-foreground mb-1">No feeds yet</p>
        <p class="text-xs">
          Add a test source or scan for NDI sources in
          <RouterLink to="/setup/sources" class="text-primary hover:underline">Setup → Sources</RouterLink>.
        </p>
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
      </div>
    </div>
  

  </div>
</template>
