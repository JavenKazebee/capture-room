<script setup lang="ts">
import { nextTick, ref } from 'vue'
import { toast } from 'vue-sonner'
import { Download, FileWarning, Loader2, Pencil, Play, Trash2 } from '@lucide/vue'
import { useReplayStore, type ReplayClip } from '@/stores/replay'
import { notifyError } from '@/lib/notify'
import { formatBytes, formatClock } from '@/lib/format'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'

/**
 * Saved replay clips on the channel's node, newest first. Click cues a clip
 * on its first frame, double-click plays it at the chosen speed. Export
 * writes it to one file (no re-encoding) in the node's Replay clips folder
 * and adds it to the media library.
 */
const props = defineProps<{
  clips: ReplayClip[]
  /** The saved clip on the channel now. */
  onAirId: string | null
}>()
const emit = defineEmits<{ cue: [clip: ReplayClip]; play: [clip: ReplayClip] }>()

const replay = useReplayStore()

/** Its capture time of day, else when it was saved. */
function timeOfDay(c: ReplayClip) {
  const d = c.start_utc_us != null ? new Date(c.start_utc_us / 1000) : new Date(c.created_at)
  return d.toLocaleTimeString([], { hour12: false, hour: '2-digit', minute: '2-digit', second: '2-digit' })
}
const length = (c: ReplayClip) => formatClock((c.to_us - c.from_us) / 1000)

// ── Rename ────────────────────────────────────────────────────────────────────

const editing = ref<string | null>(null)
const draft = ref('')
async function startRename(c: ReplayClip) {
  editing.value = c.key
  draft.value = c.name
  await nextTick()
  const el = document.getElementById('replay-clip-name') as HTMLInputElement | null
  el?.focus()
  el?.select()
}
async function commitRename(c: ReplayClip) {
  if (editing.value !== c.key) return
  editing.value = null
  const name = draft.value.trim()
  if (!name || name === c.name) return
  try {
    await replay.renameClip(c, name)
  } catch (e) {
    notifyError('Could not rename the clip', e, c.node_id)
  }
}

// ── Export and delete ─────────────────────────────────────────────────────────

const exporting = ref(new Set<string>())
async function exportClip(c: ReplayClip) {
  exporting.value.add(c.key)
  try {
    const item = await replay.exportClip(c)
    toast.success(`Exported ${c.name}`, { description: `${item.path} — now in the media library` })
  } catch (e) {
    notifyError(`Could not export ${c.name}`, e, c.node_id)
  } finally {
    exporting.value.delete(c.key)
  }
}

const deleting = ref<ReplayClip | null>(null)
async function remove() {
  const c = deleting.value
  deleting.value = null
  if (!c) return
  try {
    await replay.removeClip(c)
  } catch (e) {
    notifyError(`Could not delete ${c.name}`, e, c.node_id)
  }
}
</script>

<template>
  <section class="flex flex-col gap-2" aria-label="Clips">
    <div class="flex items-baseline gap-2">
      <h2 class="text-sm font-medium">Clips</h2>
      <span class="num text-xs text-muted-foreground">{{ props.clips.length }}</span>
      <span class="flex-1" />
      <span class="text-[11px] text-muted-foreground">S saves in → out · Shift+S on every angle</span>
    </div>

    <p v-if="!props.clips.length" class="rounded-md border border-dashed border-border px-3 py-4 text-center text-xs text-muted-foreground">
      Mark an in and an out, then save the clip (S). Saved clips are kept after the buffer rolls on.
    </p>

    <ul v-else class="rounded-md border border-border divide-y divide-border overflow-hidden">
      <li
        v-for="(c, i) in props.clips"
        :key="c.key"
        class="group flex items-center gap-2 px-2 py-1.5 text-xs cursor-pointer select-none"
        :class="c.id === props.onAirId ? 'bg-success/10' : 'hover:bg-accent/40'"
        :title="c.missing ? undefined : 'Click to cue, double-click to play'"
        @click="!c.missing && editing !== c.key && emit('cue', c)"
        @dblclick="!c.missing && editing !== c.key && emit('play', c)"
      >
        <span class="num w-6 shrink-0 text-right text-[10px] text-muted-foreground">{{ props.clips.length - i }}</span>
        <div class="flex-1 min-w-0 flex flex-col">
          <Input
            v-if="editing === c.key"
            id="replay-clip-name"
            v-model="draft"
            class="h-6 text-xs"
            aria-label="Clip name"
            @click.stop
            @dblclick.stop
            @keydown.enter.prevent="commitRename(c)"
            @keydown.esc.prevent="editing = null"
            @blur="commitRename(c)"
          />
          <span v-else class="flex items-center gap-1.5 min-w-0">
            <span class="font-medium truncate" :class="c.missing && 'line-through text-muted-foreground'">{{ c.name }}</span>
            <span
              v-if="c.id === props.onAirId"
              class="shrink-0 rounded-sm bg-primary px-1 text-[9px] font-semibold uppercase tracking-wide text-primary-foreground"
            >
              On air
            </span>
          </span>
          <span class="flex items-center gap-2 text-[11px] text-muted-foreground">
            <span class="truncate">{{ c.source_name }}</span>
            <span class="num">{{ timeOfDay(c) }}</span>
            <span class="num">{{ length(c) }}</span>
            <span class="num">{{ formatBytes(c.bytes) }}</span>
          </span>
          <span v-if="c.missing" class="flex items-center gap-1 text-[11px] text-destructive">
            <FileWarning class="size-3" /> Its files are no longer on disk
          </span>
        </div>
        <div class="flex items-center gap-0.5 shrink-0">
          <Button
            size="icon"
            variant="ghost"
            class="size-7"
            :disabled="c.missing"
            :aria-label="`Play ${c.name}`"
            title="Play at the chosen speed"
            @click.stop="emit('play', c)"
          >
            <Play class="size-3.5 fill-current" />
          </Button>
          <Button
            size="icon"
            variant="ghost"
            class="size-7"
            :disabled="c.missing || exporting.has(c.key)"
            :aria-label="`Export ${c.name}`"
            title="Export to a file and the media library"
            @click.stop="exportClip(c)"
          >
            <Loader2 v-if="exporting.has(c.key)" class="size-3.5 animate-spin" />
            <Download v-else class="size-3.5" />
          </Button>
          <Button size="icon" variant="ghost" class="size-7" :aria-label="`Rename ${c.name}`" title="Rename" @click.stop="startRename(c)">
            <Pencil class="size-3.5" />
          </Button>
          <Button
            size="icon"
            variant="ghost"
            class="size-7 text-muted-foreground hover:text-destructive"
            :aria-label="`Delete ${c.name}`"
            title="Delete"
            @click.stop="deleting = c"
          >
            <Trash2 class="size-3.5" />
          </Button>
        </div>
      </li>
    </ul>
  </section>

  <ConfirmDialog
    :open="deleting !== null"
    :title="`Delete ${deleting?.name ?? 'the clip'}?`"
    description="Its saved frames are deleted. Files exported from it stay."
    @update:open="(v) => !v && (deleting = null)"
    @confirm="remove"
  />
</template>
