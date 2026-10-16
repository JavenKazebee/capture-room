<script setup lang="ts">
import { computed, nextTick, ref } from 'vue'
import { toast } from 'vue-sonner'
import { Download, FileWarning, Loader2, Pencil, Play, Trash2 } from '@lucide/vue'
import { useReplayStore, type ReplayClip, type ReplayClipGroup } from '@/stores/replay'
import { notifyError } from '@/lib/notify'
import { clockTime, formatBytes, formatClock, plural } from '@/lib/format'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'

/**
 * Saved replay clips on the channel's node, newest first. Click cues a clip
 * on its first frame, double-click plays it at the chosen speed. Export
 * writes it to one file (no re-encoding) in the node's Replay clips folder
 * and adds it to the media library.
 *
 * Clips saved on every angle at once are one row: a group. The row cues and
 * plays the selected angle's clip; its angles below pick another, and while
 * one of the group is on air, picking another takes over at the same moment.
 */
const props = defineProps<{
  clips: ReplayClip[]
  /** The saved clip on the channel now. */
  onAirId: string | null
  /** The selected angle's source, the one a group's row plays. */
  angleSourceId: string | null
}>()
const emit = defineEmits<{ cue: [clip: ReplayClip]; play: [clip: ReplayClip] }>()

const replay = useReplayStore()

type Entry = { kind: 'clip'; key: string; clip: ReplayClip } | { kind: 'group'; key: string; group: ReplayClipGroup }

/** Clips and groups, newest first; the node lists a group's clips together. */
const entries = computed<Entry[]>(() => {
  const out: Entry[] = []
  const groups = new Map<string, ReplayClipGroup>()
  for (const c of props.clips) {
    if (!c.group_id) {
      out.push({ kind: 'clip', key: c.key, clip: c })
      continue
    }
    let g = groups.get(c.group_id)
    if (!g) {
      g = { id: c.group_id, node_id: c.node_id, key: `${c.node_id}/group/${c.group_id}`, name: c.group_name ?? '', clips: [] }
      groups.set(c.group_id, g)
      out.push({ kind: 'group', key: g.key, group: g })
    }
    g.clips.push(c)
  }
  return out
})

/** The clip a group's row plays: the selected angle's, else its first there is. */
function lead(g: ReplayClipGroup) {
  const usable = g.clips.filter((c) => !c.missing)
  return usable.find((c) => c.source_id === props.angleSourceId) ?? usable[0] ?? null
}

const onAir = (c: ReplayClip) => c.id === props.onAirId
const groupOnAir = (g: ReplayClipGroup) => g.clips.some(onAir)

/** Its capture time of day, else when it was saved. */
function timeOfDay(c: ReplayClip) {
  return clockTime(c.start_utc_us != null ? new Date(c.start_utc_us / 1000) : c.created_at)
}
const length = (c: ReplayClip) => formatClock((c.to_us - c.from_us) / 1000)
const groupBytes = (g: ReplayClipGroup) => g.clips.reduce((sum, c) => sum + c.bytes, 0)

function cue(e: Entry) {
  const c = e.kind === 'clip' ? (e.clip.missing ? null : e.clip) : lead(e.group)
  if (c && editing.value !== e.key) emit('cue', c)
}
function play(e: Entry) {
  const c = e.kind === 'clip' ? (e.clip.missing ? null : e.clip) : lead(e.group)
  if (c && editing.value !== e.key) emit('play', c)
}
const entryName = (e: Entry) => (e.kind === 'clip' ? e.clip.name : e.group.name)
const entryMissing = (e: Entry) => (e.kind === 'clip' ? e.clip.missing : !lead(e.group))

// ── Rename ────────────────────────────────────────────────────────────────────

const editing = ref<string | null>(null)
const draft = ref('')
async function startRename(e: Entry) {
  editing.value = e.key
  draft.value = entryName(e)
  await nextTick()
  const el = document.getElementById('replay-clip-name') as HTMLInputElement | null
  el?.focus()
  el?.select()
}
async function commitRename(e: Entry) {
  if (editing.value !== e.key) return
  editing.value = null
  const name = draft.value.trim()
  if (!name || name === entryName(e)) return
  try {
    if (e.kind === 'clip') await replay.renameClip(e.clip, name)
    else await replay.renameClipGroup(e.group, name)
  } catch (err) {
    notifyError(`Could not rename the ${e.kind === 'clip' ? 'clip' : 'group'}`, err, e.kind === 'clip' ? e.clip.node_id : e.group.node_id)
  }
}

// ── Export and delete ─────────────────────────────────────────────────────────

const exporting = ref(new Set<string>())
async function exportEntry(e: Entry) {
  const name = entryName(e)
  exporting.value.add(e.key)
  try {
    if (e.kind === 'clip') {
      const item = await replay.exportClip(e.clip)
      toast.success(`Exported ${name}`, { description: `${item.path} — now in the media library` })
    } else {
      const items = await replay.exportClipGroup(e.group)
      toast.success(`Exported ${name}`, {
        description: `${plural(items.length, 'file')} in the Replay clips folder — now in the media library`,
      })
    }
  } catch (err) {
    notifyError(`Could not export ${name}`, err, e.kind === 'clip' ? e.clip.node_id : e.group.node_id)
  } finally {
    exporting.value.delete(e.key)
  }
}

const deleting = ref<Entry | null>(null)
async function remove() {
  const e = deleting.value
  deleting.value = null
  if (!e) return
  try {
    if (e.kind === 'clip') await replay.removeClip(e.clip)
    else await replay.removeClipGroup(e.group)
  } catch (err) {
    notifyError(`Could not delete ${entryName(e)}`, err, e.kind === 'clip' ? e.clip.node_id : e.group.node_id)
  }
}
</script>

<template>
  <section class="flex flex-col gap-2" aria-label="Clips">
    <div class="flex items-baseline gap-2">
      <h2 class="text-sm font-medium">Clips</h2>
      <span class="num text-xs text-muted-foreground">{{ entries.length }}</span>
      <span class="flex-1" />
      <span class="text-[11px] text-muted-foreground">S saves in → out · Shift+S on every angle, as one</span>
    </div>

    <p v-if="!entries.length" class="rounded-md border border-dashed border-border px-3 py-4 text-center text-xs text-muted-foreground">
      Mark an in and an out, then save the clip (S). Saved clips are kept after the buffer rolls on.
    </p>

    <ul v-else class="rounded-md border border-border divide-y divide-border overflow-hidden">
      <li
        v-for="(e, i) in entries"
        :key="e.key"
        class="group flex items-center gap-2 px-2 py-1.5 text-xs cursor-pointer select-none"
        :class="(e.kind === 'clip' ? onAir(e.clip) : groupOnAir(e.group)) ? 'bg-success/10' : 'hover:bg-accent/40'"
        :title="entryMissing(e) ? undefined : 'Click to cue, double-click to play'"
        @click="cue(e)"
        @dblclick="play(e)"
      >
        <span class="num w-6 shrink-0 self-start pt-0.5 text-right text-[10px] text-muted-foreground">{{ entries.length - i }}</span>
        <div class="flex-1 min-w-0 flex flex-col">
          <Input
            v-if="editing === e.key"
            id="replay-clip-name"
            v-model="draft"
            class="h-6 text-xs"
            :aria-label="e.kind === 'clip' ? 'Clip name' : 'Group name'"
            @click.stop
            @dblclick.stop
            @keydown.enter.prevent="commitRename(e)"
            @keydown.esc.prevent="editing = null"
            @blur="commitRename(e)"
          />
          <span v-else class="flex items-center gap-1.5 min-w-0">
            <span class="font-medium truncate" :class="entryMissing(e) && 'line-through text-muted-foreground'">{{ entryName(e) }}</span>
            <span
              v-if="e.kind === 'clip' ? onAir(e.clip) : groupOnAir(e.group)"
              class="shrink-0 rounded-sm bg-primary px-1 text-[9px] font-semibold uppercase tracking-wide text-primary-foreground"
            >
              On air
            </span>
          </span>

          <template v-if="e.kind === 'clip'">
            <span class="flex items-center gap-2 text-[11px] text-muted-foreground">
              <span class="truncate">{{ e.clip.source_name }}</span>
              <span class="num">{{ timeOfDay(e.clip) }}</span>
              <span class="num">{{ length(e.clip) }}</span>
              <span class="num">{{ formatBytes(e.clip.bytes) }}</span>
            </span>
            <span v-if="e.clip.missing" class="flex items-center gap-1 text-[11px] text-destructive">
              <FileWarning class="size-3" /> Its files are no longer on disk
            </span>
          </template>

          <template v-else>
            <span class="flex items-center gap-2 text-[11px] text-muted-foreground">
              <span>{{ plural(e.group.clips.length, 'angle') }}</span>
              <span class="num">{{ timeOfDay(e.group.clips[0]!) }}</span>
              <span class="num">{{ length(e.group.clips[0]!) }}</span>
              <span class="num">{{ formatBytes(groupBytes(e.group)) }}</span>
            </span>
            <span class="mt-1 flex flex-wrap gap-1" role="group" :aria-label="`Angles of ${e.group.name}`">
              <button
                v-for="c in e.group.clips"
                :key="c.key"
                type="button"
                class="max-w-40 truncate rounded-sm border px-1.5 py-px text-[11px] leading-4 disabled:cursor-not-allowed disabled:line-through disabled:opacity-60"
                :class="
                  onAir(c)
                    ? 'border-primary bg-primary text-primary-foreground'
                    : c.source_id === props.angleSourceId
                      ? 'border-foreground/40 bg-background'
                      : 'border-border bg-background hover:bg-accent'
                "
                :disabled="c.missing"
                :title="c.missing ? 'Its files are no longer on disk' : groupOnAir(e.group) ? 'Click to switch at the same moment, double-click to play from the start' : 'Click to cue, double-click to play'"
                @click.stop="emit('cue', c)"
                @dblclick.stop="emit('play', c)"
              >
                {{ c.name }}
              </button>
            </span>
          </template>
        </div>
        <div class="flex items-center gap-0.5 shrink-0">
          <Button
            size="icon"
            variant="ghost"
            class="size-7"
            :disabled="entryMissing(e)"
            :aria-label="`Play ${entryName(e)}`"
            :title="e.kind === 'clip' ? 'Play at the chosen speed' : 'Play the selected angle at the chosen speed'"
            @click.stop="play(e)"
          >
            <Play class="size-3.5 fill-current" />
          </Button>
          <Button
            size="icon"
            variant="ghost"
            class="size-7"
            :disabled="entryMissing(e) || exporting.has(e.key)"
            :aria-label="`Export ${entryName(e)}`"
            :title="e.kind === 'clip' ? 'Export to a file and the media library' : 'Export every angle, a file each, to the media library'"
            @click.stop="exportEntry(e)"
          >
            <Loader2 v-if="exporting.has(e.key)" class="size-3.5 animate-spin" />
            <Download v-else class="size-3.5" />
          </Button>
          <Button size="icon" variant="ghost" class="size-7" :aria-label="`Rename ${entryName(e)}`" title="Rename" @click.stop="startRename(e)">
            <Pencil class="size-3.5" />
          </Button>
          <Button
            size="icon"
            variant="ghost"
            class="size-7 text-muted-foreground hover:text-destructive"
            :aria-label="`Delete ${entryName(e)}`"
            title="Delete"
            @click.stop="deleting = e"
          >
            <Trash2 class="size-3.5" />
          </Button>
        </div>
      </li>
    </ul>
  </section>

  <ConfirmDialog
    :open="deleting !== null"
    :title="`Delete ${deleting ? entryName(deleting) : 'the clip'}?`"
    :description="
      deleting?.kind === 'group'
        ? `Every angle's saved frames are deleted (${plural(deleting.group.clips.length, 'clip')}). Files exported from them stay.`
        : 'Its saved frames are deleted. Files exported from it stay.'
    "
    @update:open="(v) => !v && (deleting = null)"
    @confirm="remove"
  />
</template>
