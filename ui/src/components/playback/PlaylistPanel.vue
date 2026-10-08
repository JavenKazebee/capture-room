<script setup lang="ts">
import { computed, ref } from 'vue'
import { ArrowDown, FileWarning, GripVertical, Pause, Repeat, Square, X } from '@lucide/vue'
import { usePlayoutStore } from '@/stores/playout'
import type { Source } from '@/stores/sources'
import { notifyError } from '@/lib/notify'
import { formatClock } from '@/lib/format'
import type { ChannelStatusDto } from '@/types/generated/ChannelStatusDto'
import type { ClipEnd } from '@/types/generated/ClipEnd'
import type { PlaylistItemDto } from '@/types/generated/PlaylistItemDto'
import type { PlaylistItemInput } from '@/types/generated/PlaylistItemInput'
import { Switch } from '@/components/ui/switch'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { MEDIA_DRAG } from './transport'

/**
 * A channel's playlist. Click selects an item, double-click (or Enter) cues
 * it. Rows drag to reorder, and media dragged in from the library is added
 * where it's dropped. Each edit saves the whole list.
 */
const props = defineProps<{ channel: Source; status?: ChannelStatusDto }>()
const selected = defineModel<string | null>('selected', { required: true })
const emit = defineEmits<{ cue: [item: PlaylistItemDto] }>()

const playout = usePlayoutStore()
const playlist = computed(() => playout.playlists.get(props.channel.key))
const items = computed(() => playlist.value?.items ?? [])

const END: Record<ClipEnd, { label: string; icon: typeof ArrowDown; hint: string }> = {
  next: { label: 'Next', icon: ArrowDown, hint: 'Plays the next item without a gap' },
  hold: { label: 'Hold', icon: Pause, hint: 'Holds the last frame' },
  black: { label: 'Black', icon: Square, hint: 'Goes to black' },
  loop: { label: 'Loop', icon: Repeat, hint: 'Loops until you take the next item' },
}

/** How long an item plays, if known. */
function length(i: PlaylistItemDto): number | null {
  const out = i.out_ms ?? i.duration_ms
  return out == null ? null : Math.max(0, out - i.in_ms)
}
const total = computed(() => {
  const lengths = items.value.map(length)
  return lengths.some((l) => l == null) ? null : lengths.reduce<number>((a, l) => a + (l ?? 0), 0)
})

function input(i: PlaylistItemDto): PlaylistItemInput {
  return { id: i.id, media_id: i.media_id, in_ms: i.in_ms, out_ms: i.out_ms, end: i.end }
}

async function save(list: PlaylistItemInput[], loopPlaylist = playlist.value?.loop_playlist ?? false) {
  try {
    return await playout.savePlaylist(props.channel.node_id, props.channel.id, list, loopPlaylist)
  } catch (e) {
    notifyError('Could not save the playlist', e, props.channel.node_id)
  }
}

/** Add a library entry at `index` (the end by default), and select it. */
async function add(mediaId: string, index = items.value.length) {
  const list = items.value.map(input)
  list.splice(index, 0, { id: null, media_id: mediaId, in_ms: 0, out_ms: null, end: 'next' })
  const p = await save(list)
  if (p) selected.value = p.items[index]?.id ?? selected.value
}

async function remove(i: PlaylistItemDto) {
  await save(items.value.filter((x) => x.id !== i.id).map(input))
  if (selected.value === i.id) selected.value = null
}

function setLoop(on: boolean) {
  save(items.value.map(input), on)
}

// ── Drag and drop ─────────────────────────────────────────────────────────────

const ITEM_DRAG = 'application/x-cr-playlist-item'
/** Where a drop would land: before this index. */
const dropAt = ref<number | null>(null)

function dragStart(e: DragEvent, i: PlaylistItemDto) {
  e.dataTransfer?.setData(ITEM_DRAG, i.id)
  if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move'
}

function accepts(e: DragEvent) {
  const types = e.dataTransfer?.types ?? []
  return types.includes(ITEM_DRAG) || types.includes(MEDIA_DRAG)
}

function dragOver(e: DragEvent, index: number) {
  if (!accepts(e)) return
  e.preventDefault()
  // The lower half of a row drops after it.
  const row = e.currentTarget as HTMLElement
  const r = row.getBoundingClientRect()
  dropAt.value = index < items.value.length && e.clientY > r.top + r.height / 2 ? index + 1 : index
}

async function drop(e: DragEvent) {
  const at = dropAt.value
  dropAt.value = null
  if (at === null || !e.dataTransfer) return
  e.preventDefault()
  const mediaId = e.dataTransfer.getData(MEDIA_DRAG)
  if (mediaId) return add(mediaId, at)
  const id = e.dataTransfer.getData(ITEM_DRAG)
  const from = items.value.findIndex((i) => i.id === id)
  if (from < 0 || at === from || at === from + 1) return
  const list = items.value.map(input)
  const [moved] = list.splice(from, 1)
  list.splice(at > from ? at - 1 : at, 0, moved!)
  save(list)
}

defineExpose({ add, remove })
</script>

<template>
  <section class="rounded-lg border border-border flex flex-col min-h-0">
    <div class="flex items-center gap-2 px-3 h-10 border-b border-border shrink-0">
      <span class="text-xs font-semibold">Playlist</span>
      <span class="num text-xs text-muted-foreground">{{ items.length }}</span>
      <span v-if="items.length && total != null" class="num text-xs text-muted-foreground">· {{ formatClock(total) }}</span>
      <div class="flex-1" />
      <label class="flex items-center gap-2 text-xs text-muted-foreground cursor-pointer">
        Loop the playlist
        <Switch
          :model-value="playlist?.loop_playlist ?? false"
          :disabled="!playlist"
          @update:model-value="setLoop"
        />
      </label>
    </div>

    <ol
      class="min-h-24 divide-y divide-border/60"
      role="listbox"
      aria-label="Playlist"
      @dragover="(e) => dragOver(e, items.length)"
      @dragleave.self="dropAt = null"
      @drop="drop"
    >
      <li
        v-for="(i, n) in items"
        :key="i.id"
        role="option"
        :aria-selected="selected === i.id"
        tabindex="0"
        draggable="true"
        class="group relative flex items-center gap-2 pl-1 pr-2 py-1.5 cursor-pointer outline-none focus-visible:bg-accent/60"
        :class="[
          selected === i.id ? 'bg-accent' : 'hover:bg-accent/40',
          status?.item_id === i.id && 'shadow-[inset_3px_0_0_var(--color-success)]',
        ]"
        @click="selected = i.id"
        @dblclick="!i.missing && emit('cue', i)"
        @keydown.enter.prevent="!i.missing && emit('cue', i)"
        @keydown.delete.prevent="remove(i)"
        @dragstart="(e) => dragStart(e, i)"
        @dragover.stop="(e) => dragOver(e, n)"
        @dragend="dropAt = null"
      >
        <div v-if="dropAt === n" class="absolute inset-x-0 -top-px h-0.5 bg-primary pointer-events-none" />
        <div
          v-if="dropAt === n + 1 && n === items.length - 1"
          class="absolute inset-x-0 -bottom-px h-0.5 bg-primary pointer-events-none"
        />
        <GripVertical class="size-3.5 shrink-0 text-muted-foreground/50 group-hover:text-muted-foreground cursor-grab" />
        <span class="num w-5 shrink-0 text-right text-[11px] text-muted-foreground">{{ n + 1 }}</span>
        <div class="flex-1 min-w-0">
          <div class="flex items-center gap-1.5 min-w-0">
            <span class="text-sm truncate" :class="i.missing && 'text-muted-foreground line-through'">{{ i.name }}</span>
            <span
              v-if="status?.item_id === i.id"
              class="shrink-0 rounded-sm bg-success px-1 text-[9px] font-semibold uppercase tracking-wide text-black"
            >
              On air
            </span>
            <span
              v-else-if="status?.next_id === i.id"
              class="shrink-0 rounded-sm border border-primary px-1 text-[9px] font-semibold uppercase tracking-wide text-primary"
              :title="status.next_ready ? 'Cued in the background' : 'Cueing in the background'"
            >
              Next{{ status.next_ready ? '' : '…' }}
            </span>
          </div>
          <div class="flex items-center gap-2 text-[11px] text-muted-foreground num">
            <span v-if="i.in_ms || i.out_ms != null">
              {{ formatClock(i.in_ms) }} – {{ i.out_ms != null ? formatClock(i.out_ms) : 'end' }}
            </span>
            <span v-else class="font-sans">Whole file</span>
            <span v-if="i.missing" class="flex items-center gap-1 text-destructive font-sans">
              <FileWarning class="size-3" /> File no longer on disk
            </span>
          </div>
        </div>
        <span class="num w-14 shrink-0 text-right text-xs">{{ length(i) != null ? formatClock(length(i)!) : '—' }}</span>
        <Tooltip>
          <TooltipTrigger as-child>
            <span class="w-14 shrink-0 flex items-center gap-1 text-[11px] text-muted-foreground">
              <component :is="END[i.end].icon" class="size-3" /> {{ END[i.end].label }}
            </span>
          </TooltipTrigger>
          <TooltipContent>At the out point: {{ END[i.end].hint.toLowerCase() }}</TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger as-child>
            <button
              class="size-6 grid place-items-center rounded text-muted-foreground opacity-0 group-hover:opacity-100 focus-visible:opacity-100 hover:text-foreground hover:bg-accent"
              :aria-label="`Remove ${i.name} from the playlist`"
              @click.stop="remove(i)"
            >
              <X class="size-3.5" />
            </button>
          </TooltipTrigger>
          <TooltipContent>Remove from the playlist</TooltipContent>
        </Tooltip>
      </li>
      <li
        v-if="!items.length"
        class="px-3 py-6 text-center text-xs text-muted-foreground"
        :class="dropAt !== null && 'bg-primary/10'"
      >
        Add clips from the media list: double-click one, or drag it here.
      </li>
    </ol>
  </section>
</template>
