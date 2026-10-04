<script setup lang="ts">
import { computed, ref } from 'vue'
import { useEventListener } from '@vueuse/core'
import { Eye, PanelRight, Search } from '@lucide/vue'
import { useSourcesStore } from '@/stores/sources'
import { useNodesStore } from '@/stores/nodes'
import { useRecordingsStore } from '@/stores/recordings'
import { useRecordDeskStore, type StateFilter } from '@/stores/recordDesk'
import { usePreferences, type TileOverlays } from '@/composables/usePreferences'
import { wsStatus } from '@/composables/useWebSocket'
import { shortcut } from '@/lib/keys'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Slider } from '@/components/ui/slider'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from '@/components/ui/resizable'
import OptionSelect from '@/components/OptionSelect.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import FeedTile from '@/components/record/FeedTile.vue'
import RecordInspector from '@/components/record/RecordInspector.vue'

const sources = useSourcesStore()
const nodes = useNodesStore()
const recordings = useRecordingsStore()
const desk = useRecordDeskStore()
const { tileSize, overlays, inspectorOpen } = usePreferences()

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

const tileSizeModel = computed({
  get: () => [tileSize.value],
  set: (v: number[] | undefined) => v?.[0] && (tileSize.value = v[0]),
})

const live = computed(() => recordings.activeSessions.length)
const selectedLive = computed(() => desk.selectedSources.filter((s) => desk.isLive(s)))
const selectedIdle = computed(() => desk.selectedSources.filter((s) => !desk.isLive(s)))

// Stop is confirmed; what it applies to depends on whether anything is selected.
const confirmStop = ref<'selected' | 'all' | null>(null)
const stopTargets = computed(() =>
  confirmStop.value === 'all' ? sources.sources.filter((s) => desk.isLive(s)) : selectedLive.value,
)

// ── Keyboard ──────────────────────────────────────────────────────────────────

useEventListener('keydown', (e: KeyboardEvent) => {
  const t = e.target as HTMLElement
  if (t.closest('input, textarea, select, [role="dialog"], [role="listbox"], [role="menu"]')) return
  if (e.key === 'Escape') desk.clearSelection()
  else if (e.key.toLowerCase() === 'a' && (e.ctrlKey || e.metaKey)) {
    e.preventDefault()
    desk.selectAll()
  }
})
</script>

<template>
  <div class="h-full flex flex-col min-h-0">
    <PageHeader title="Record">
      <template #meta>
        <span class="num text-xs text-muted-foreground">
          {{ desk.visible.length }}<template v-if="desk.filtered">/{{ sources.sources.length }}</template> feeds
        </span>
        <span v-if="live" class="num text-xs text-tally font-medium">· {{ live }} live</span>
      </template>

      <!-- Filters -->
      <div class="relative">
        <Search class="absolute left-2 top-1/2 -translate-y-1/2 size-3.5 text-muted-foreground" />
        <Input v-model="desk.query" placeholder="Filter feeds…" class="h-7 w-40 pl-7 text-xs" />
      </div>
      <OptionSelect v-if="nodes.nodes.length > 1" v-model="desk.nodeFilter" :options="nodeOptions" class="h-7 w-32 text-xs" />
      <OptionSelect v-if="typeOptions.length > 2" v-model="desk.typeFilter" :options="typeOptions" class="h-7 w-28 text-xs" />
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

      <div class="w-px h-5 bg-border mx-1" />

      <!-- View -->
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
      <div class="flex items-center gap-2 w-32" title="Tile size">
        <span class="text-[10px] text-muted-foreground">Size</span>
        <Slider v-model="tileSizeModel" :min="200" :max="640" :step="20" />
      </div>
      <Button
        variant="outline"
        size="sm"
        class="h-7 w-7 p-0"
        :class="inspectorOpen && 'text-primary border-primary/50'"
        :title="inspectorOpen ? 'Hide inspector' : 'Show inspector'"
        @click="inspectorOpen = !inspectorOpen"
      >
        <PanelRight class="size-3.5" />
      </Button>
    </PageHeader>

    <!-- Selection bar: always present so selecting never shifts the grid -->
    <div
      class="h-10 shrink-0 flex items-center gap-2 px-4 border-b border-border text-xs transition-colors"
      :class="desk.selected.size ? 'bg-primary/10' : 'bg-card'"
    >
      <template v-if="desk.selected.size">
        <span class="font-medium text-primary">{{ desk.selected.size }} selected</span>
        <button class="text-muted-foreground hover:text-foreground hover:underline underline-offset-2" @click="desk.clearSelection()">Clear</button>
        <button class="text-muted-foreground hover:text-foreground hover:underline underline-offset-2" @click="desk.selectAll()">Select all visible</button>
        <div class="flex-1" />
        <span class="text-muted-foreground">Preset</span>
        <OptionSelect
          :model-value="desk.selectedPresetId"
          :options="desk.selectionPresetOptions"
          class="h-7 w-44 text-xs"
          @update:model-value="desk.setSelectionPreset"
        />
        <Button variant="outline" size="sm" class="h-7 gap-1.5 text-xs" :disabled="!selectedIdle.length" @click="desk.bulk('start', selectedIdle)">
          <span class="size-2 rounded-full bg-tally" /> Record {{ selectedIdle.length }}
        </Button>
        <Button variant="outline" size="sm" class="h-7 gap-1.5 text-xs" :disabled="!selectedLive.length" @click="confirmStop = 'selected'">
          <span class="size-2 rounded-[1px] bg-tally" /> Stop {{ selectedLive.length }}
        </Button>
      </template>
      <template v-else>
        <span class="text-muted-foreground">Click a tile to inspect it · {{ shortcut('Click') }} or Shift+Click to select several · {{ shortcut('A') }} for all</span>
        <div class="flex-1" />
        <Button v-if="live" variant="outline" size="sm" class="h-7 gap-1.5 text-xs" @click="confirmStop = 'all'">
          <span class="size-2 rounded-[1px] bg-tally" /> Stop all ({{ live }})
        </Button>
      </template>
    </div>

    <ResizablePanelGroup direction="horizontal" auto-save-id="cr.record.layout" class="flex-1 min-h-0">
      <ResizablePanel id="grid" :order="1" :min-size="35">
        <div
          class="h-full overflow-y-auto p-4"
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
            :style="{ gridTemplateColumns: `repeat(auto-fill, minmax(min(100%, ${tileSize}px), ${tileSize}px))` }"
            @click.self="desk.clearSelection()"
          >
            <FeedTile v-for="source in desk.visible" :key="source.key" :source="source" />
          </div>
        </div>
      </ResizablePanel>

      <template v-if="inspectorOpen">
        <ResizableHandle />
        <ResizablePanel id="inspector" :order="2" :default-size="28" :min-size="18" :max-size="50">
          <RecordInspector @close="inspectorOpen = false" />
        </ResizablePanel>
      </template>
    </ResizablePanelGroup>

    <ConfirmDialog
      :open="!!confirmStop"
      :title="`Stop ${stopTargets.length} recording${stopTargets.length === 1 ? '' : 's'}?`"
      :description="confirmStop === 'all' ? 'Every live recording on every node stops now. Files written so far are kept.' : 'The selected feeds stop recording now. Files written so far are kept.'"
      confirm-label="Stop recording"
      @update:open="(v) => !v && (confirmStop = null)"
      @confirm="desk.bulk('stop', stopTargets)"
    />
  </div>
</template>
