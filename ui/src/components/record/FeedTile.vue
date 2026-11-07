<script setup lang="ts">
import { computed, ref, toRef, watch } from 'vue'
import { Check, Eye, EyeOff, Info, Tag } from '@lucide/vue'
import { useSourcesStore, type Source } from '@/stores/sources'
import { totalDropped, useRecordingsStore } from '@/stores/recordings'
import { useRecordDeskStore } from '@/stores/recordDesk'
import { useNodesStore } from '@/stores/nodes'
import { useLayoutStore } from '@/stores/layout'
import { useEventsStore } from '@/stores/events'
import { usePreferences } from '@/composables/usePreferences'
import { useThumbnail } from '@/composables/useThumbnail'
import { useFeedState } from './useFeedState'
import { ltcWarning } from '@/lib/ltc'
import { Button } from '@/components/ui/button'
import { Popover, PopoverAnchor, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import AudioMeter from '@/components/AudioMeter.vue'
import PresetSelect from './PresetSelect.vue'
import FeedInfo from './FeedInfo.vue'
import TagEditor from './TagEditor.vue'
import TallyBadge from '@/components/common/TallyBadge.vue'

/**
 * `tabStop`: the one tile Tab lands on (roving tabindex). `dragging`: this tile is being moved.
 * `dropSide`: a dragged tile would land here, before or after this one.
 */
const props = defineProps<{ source: Source; tabStop?: boolean; dragging?: boolean; dropSide?: 'before' | 'after' | null }>()
const emit = defineEmits<{ expand: [] }>()

const desk = useRecordDeskStore()
const sources = useSourcesStore()
const recordings = useRecordingsStore()
const nodes = useNodesStore()
const layout = useLayoutStore()
const tags = computed(() => layout.tagsOf(props.source.key))
const { overlays } = usePreferences()

const { session, busy, channels, preset, duration, noSignal, format } = useFeedState(toRef(props, 'source'))
const ltc = computed(() => ltcWarning(props.source.ltc))
const selected = computed(() => desk.selected.has(props.source.key))
const tile = ref<HTMLElement | null>(null)
const picture = ref<HTMLElement | null>(null)
const thumb = useThumbnail(toRef(props, 'source'), picture)

const infoOpen = computed({
  get: () => desk.infoKey === props.source.key,
  set: (open: boolean) => {
    if (open) desk.infoKey = props.source.key
    else if (desk.infoKey === props.source.key) desk.infoKey = null
  },
})

function onClick(e: MouseEvent) {
  if (e.shiftKey) desk.selectRange(props.source.key)
  else desk.select(props.source.key, e.ctrlKey || e.metaKey ? 'toggle' : 'only')
}

// Double-clicking a control (Record, the preset) is just two clicks on it.
function onDblClick(e: MouseEvent) {
  if ((e.target as HTMLElement).closest('button, input, select, [role="combobox"]')) return
  emit('expand')
}

// Tabbing onto a tile focuses it, as arrowing does; focus on its own controls doesn't.
function onFocusIn(e: FocusEvent) {
  if (e.target === e.currentTarget) desk.focusedKey = props.source.key
}

const tagsOpen = computed({
  get: () => desk.tagsKey === props.source.key,
  set: (open: boolean) => {
    if (open) {
      desk.infoKey = null
      desk.tagsKey = props.source.key
    } else if (desk.tagsKey === props.source.key) desk.tagsKey = null
  },
})
// Kept while it's open, so a click that changes the selection doesn't retarget it.
const tagKeys = ref<string[]>([])
watch(tagsOpen, (open) => open && (tagKeys.value = desk.tagScope(props.source.key)), { immediate: true })

// Opening tags goes straight to the new-tag field.
function onTagsOpened(e: Event) {
  e.preventDefault()
  ;(e.target as HTMLElement | null)?.querySelector<HTMLInputElement>('input')?.focus()
}

// Closing the info popover hands focus back to the tile, not its Info button, so Space still records.
// A click elsewhere that closed it keeps the focus it gave.
function onInfoClosed(e: Event) {
  e.preventDefault()
  const a = document.activeElement
  if (!a || a === document.body || tile.value?.contains(a) || a.closest('[data-slot="popover-content"]')) {
    tile.value?.focus({ preventScroll: true })
  }
}

// A session that ends on its own in error is worth a log line.
watch(session, (now, prev) => {
  if (now || !prev) return
  const ended = recordings.find(prev.node_id, prev.id)
  if (ended?.status === 'error' && ended.error_message) {
    useEventsStore().log('error', `Recording ended: ${props.source.display_name}`, {
      node_id: prev.node_id,
      detail: ended.error_message,
    })
  }
})
</script>

<template>
  <Popover v-model:open="infoOpen">
    <PopoverAnchor as-child>
      <div
        ref="tile"
        data-feed-tile
        :data-key="source.key"
        :tabindex="tabStop ? 0 : -1"
        :aria-label="source.display_name"
        class="group relative rounded-lg border bg-card overflow-hidden flex flex-col cursor-pointer select-none transition-shadow focus-visible:shadow-[0_0_0_6px_var(--background),0_0_0_8px_var(--foreground)]"
        draggable="true"
        :class="[
          session ? 'border-tally ring-1 ring-tally' : 'border-border hover:border-foreground/20',
          dragging && 'opacity-40',
          // Selection owns the outline; focus is the wider ring drawn outside it.
          selected ? 'outline-2 outline-offset-2 outline-primary' : 'outline-none',
        ]"
        @focusin="onFocusIn"
        @click="onClick"
        @dblclick="onDblClick"
      >
        <span
          v-if="dropSide"
          class="pointer-events-none absolute inset-y-0 z-10 w-1 bg-primary"
          :class="dropSide === 'before' ? 'left-0' : 'right-0'"
        />
        <div ref="picture" class="relative flex bg-black aspect-video">
          <div class="flex-1 relative overflow-hidden">
            <div
              v-if="source.error"
              class="absolute inset-0 flex flex-col items-center justify-center gap-1 px-4 text-center text-xs"
            >
              <span class="text-destructive font-medium">Source failed</span>
              <span class="text-zinc-400 line-clamp-3">{{ source.error }}</span>
            </div>
            <div
              v-else-if="source.thumbnail_hidden"
              class="absolute inset-0 flex flex-col items-center justify-center gap-1.5 text-zinc-500 text-xs"
            >
              <EyeOff class="size-4" />
              Thumbnail hidden
              <button
                class="rounded px-2 py-0.5 text-zinc-300 hover:text-white hover:bg-white/10"
                @click.stop="sources.setThumbnailHidden(source, false)"
              >
                Show
              </button>
            </div>
            <img
              v-else-if="thumb.src.value"
              :src="thumb.src.value"
              :alt="source.display_name"
              class="w-full h-full object-cover"
              draggable="false"
              @error="thumb.onError"
            />
            <div v-else class="absolute inset-0 grid place-items-center text-zinc-500 text-xs">No signal</div>
            <div
              v-if="noSignal"
              class="absolute inset-0 flex flex-col items-center justify-center gap-0.5 px-4 text-center text-xs bg-black/60"
            >
              <span :class="noSignal.dot === 'warn' ? 'text-warning font-medium' : 'text-zinc-300'">{{ noSignal.message }}</span>
              <span v-if="session" class="text-zinc-400">Recording black and silence</span>
            </div>

            <button
              class="absolute top-1.5 left-1.5 size-5 rounded-[4px] border grid place-items-center transition-opacity"
              :class="
                selected
                  ? 'bg-primary border-primary text-primary-foreground opacity-100'
                  : 'bg-black/50 border-white/40 text-transparent opacity-0 group-hover:opacity-100'
              "
              :aria-label="selected ? 'Deselect' : 'Select'"
              @click.stop="desk.select(source.key, 'toggle')"
            >
              <Check class="size-3.5" />
            </button>

            <div v-if="session || noSignal || ltc" class="absolute top-1.5 right-1.5 flex flex-col items-end gap-1">
              <TallyBadge v-if="session" :duration="duration" />
              <span
                v-if="noSignal"
                class="rounded-sm px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide"
                :class="noSignal.dot === 'warn' ? 'bg-warning text-black' : 'bg-black/70 text-zinc-300'"
              >
                {{ noSignal.short }}
              </span>
              <span
                v-if="ltc"
                class="rounded-sm bg-warning px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-black"
                :title="ltc.title"
              >
                {{ ltc.short }}
              </span>
              <span
                v-if="session && totalDropped(session)"
                class="rounded-sm bg-warning px-1.5 py-0.5 text-[10px] font-semibold text-black num"
                title="Video frames dropped because an encoder couldn't keep up"
              >
                {{ totalDropped(session).toLocaleString() }} dropped
              </span>
            </div>

            <!-- Timecode (bottom-left) / format (bottom-right) -->
            <div
              v-if="overlays.timecode && source.timecode"
              class="absolute bottom-1.5 left-1.5 bg-black/70 text-white text-[10px] num px-1.5 py-0.5 rounded"
            >
              {{ source.timecode }}
            </div>
            <div
              v-if="overlays.format && format"
              class="absolute bottom-1.5 right-1.5 bg-black/70 text-white text-[10px] num px-1.5 py-0.5 rounded"
            >
              {{ format }}
            </div>
          </div>

          <div v-if="overlays.meters && channels.length > 0" class="w-8 py-1 shrink-0">
            <AudioMeter :channels="channels" />
          </div>
        </div>

        <div class="px-2.5 py-2 flex flex-col gap-1.5">
          <div class="flex items-center justify-between gap-2 min-w-0">
            <span class="text-sm font-medium truncate">{{ source.display_name }}</span>
            <span class="flex items-center gap-1 shrink-0">
              <span class="text-[10px] text-muted-foreground uppercase tracking-wide">
                <template v-if="overlays.node && nodes.nodes.length > 1">{{ nodes.nameOf(source.node_id) }} · </template>{{ source.source_type }}
              </span>
              <Popover v-model:open="tagsOpen">
                <PopoverTrigger as-child>
                  <button
                    class="h-6 min-w-6 px-1 flex items-center justify-center gap-0.5 rounded text-muted-foreground hover:text-foreground hover:bg-accent"
                    :class="[tags.length && 'text-primary', tagsOpen && 'text-primary bg-accent']"
                    :aria-label="`Tags for ${source.display_name}`"
                    :title="tags.length ? `Tags: ${tags.join(', ')} (T)` : 'Tags (T)'"
                    @click.stop
                  >
                    <Tag class="size-3.5" />
                    <span v-if="tags.length" class="text-[10px] num">{{ tags.length }}</span>
                  </button>
                </PopoverTrigger>
                <PopoverContent
                  side="bottom"
                  align="end"
                  :collision-padding="12"
                  class="w-72 p-3"
                  @open-auto-focus="onTagsOpened"
                  @close-auto-focus="onInfoClosed"
                >
                  <TagEditor :keys="tagKeys" />
                </PopoverContent>
              </Popover>
              <button
                class="size-6 grid place-items-center rounded text-muted-foreground hover:text-foreground hover:bg-accent"
                :aria-label="source.thumbnail_hidden ? 'Show thumbnail' : 'Hide thumbnail'"
                :title="source.thumbnail_hidden ? 'Show thumbnail' : 'Hide thumbnail (saves the node making it)'"
                @click.stop="sources.setThumbnailHidden(source, !source.thumbnail_hidden)"
              >
                <Eye v-if="source.thumbnail_hidden" class="size-3.5" />
                <EyeOff v-else class="size-3.5" />
              </button>
              <PopoverTrigger as-child>
                <button
                  class="size-6 -mr-1 grid place-items-center rounded text-muted-foreground hover:text-foreground hover:bg-accent"
                  :class="infoOpen && 'text-primary bg-accent'"
                  :aria-label="`Details for ${source.display_name}`"
                  title="Details (I)"
                  @click.stop
                >
                  <Info class="size-3.5" />
                </button>
              </PopoverTrigger>
            </span>
          </div>

          <div class="flex gap-1.5 items-center" @click.stop>
            <PresetSelect
              v-model="preset"
              :feeds="[source.key]"
              :options="desk.presetOptions"
              :disabled="!!session || busy"
              class="h-7 text-xs flex-1 min-w-0"
            />
            <Button
              :variant="session ? 'default' : 'outline'"
              size="sm"
              class="h-7 px-3 text-xs shrink-0 gap-1.5"
              :class="session && 'bg-tally text-tally-foreground hover:bg-tally/85'"
              :disabled="busy"
              @click="desk.toggle(source)"
            >
              <span v-if="session" class="size-2 rounded-[1px] bg-current" />
              <span v-else class="size-2 rounded-full bg-tally" />
              {{ session ? 'Stop' : 'Record' }}
            </Button>
          </div>

          <p v-if="session?.error_message" class="text-xs text-destructive break-words">
            Output failed: {{ session.error_message }}
          </p>
        </div>
      </div>
    </PopoverAnchor>
    <PopoverContent side="right" align="start" :side-offset="8" :collision-padding="12" class="w-[34rem] p-3" @close-auto-focus="onInfoClosed">
      <FeedInfo :source="source" />
    </PopoverContent>
  </Popover>
</template>
