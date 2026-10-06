<script setup lang="ts">
import { computed, ref } from 'vue'
import { toast } from 'vue-sonner'
import { FileWarning, Plus, X } from '@lucide/vue'
import { usePlayoutStore } from '@/stores/playout'
import { notifyError } from '@/lib/notify'
import { formatClock } from '@/lib/format'
import { fpsLabel } from '@/lib/sourceFormat'
import type { MediaItemDto } from '@/types/generated/MediaItemDto'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import ToolbarSearch from '@/components/common/ToolbarSearch.vue'
import FileBrowseDialog from '@/components/sources/FileBrowseDialog.vue'

/**
 * A node's media library. Click picks a clip; double-click (or Enter) cues
 * it on the channel. Removing an entry leaves its file on disk.
 */
const props = defineProps<{
  nodeId: string
  /** The clip on the channel now, marked in the list. */
  loadedId: string | null
}>()
const selected = defineModel<string | null>('selected', { required: true })
const emit = defineEmits<{ cue: [item: MediaItemDto] }>()

const playout = usePlayoutStore()
const filter = ref('')
const browsing = ref(false)

const items = computed(() => {
  const q = filter.value.trim().toLowerCase()
  const all = playout.media.get(props.nodeId) ?? []
  return q ? all.filter((m) => m.name.toLowerCase().includes(q) || m.path.toLowerCase().includes(q)) : all
})

function summary(m: MediaItemDto) {
  const i = m.info
  const parts = [`${i.height}p${fpsLabel([i.fps_num, i.fps_den], true)}`]
  parts.push(i.audio_channels ? `${i.audio_channels} ch` : 'no audio')
  return parts.join(' · ')
}

async function importFile(path: string) {
  try {
    const item = await playout.addMedia(props.nodeId, path)
    selected.value = item.id
    toast.success(`Added ${item.name}`)
  } catch (e) {
    notifyError('Could not add the file', e, props.nodeId)
  }
}

async function remove(m: MediaItemDto) {
  try {
    await playout.removeMedia(props.nodeId, m.id)
    if (selected.value === m.id) selected.value = null
  } catch (e) {
    notifyError(`Could not remove ${m.name}`, e, props.nodeId)
  }
}
</script>

<template>
  <div class="flex flex-col min-h-0 h-full">
    <div class="flex items-center gap-2 px-3 h-10 border-b border-border shrink-0">
      <span class="text-xs font-semibold">Media</span>
      <span class="num text-xs text-muted-foreground">{{ playout.media.get(nodeId)?.length ?? 0 }}</span>
      <div class="flex-1" />
      <Button size="sm" variant="outline" class="h-7 gap-1.5 text-xs" @click="browsing = true">
        <Plus class="size-3.5" /> Import
      </Button>
    </div>
    <div class="px-3 py-2 border-b border-border shrink-0">
      <ToolbarSearch v-model="filter" class="w-full" />
    </div>

    <ul class="flex-1 min-h-0 overflow-y-auto divide-y divide-border/60" role="listbox" aria-label="Media">
      <li
        v-for="m in items"
        :key="m.id"
        role="option"
        :aria-selected="selected === m.id"
        tabindex="0"
        class="group flex items-start gap-2 px-3 py-2 cursor-pointer outline-none focus-visible:bg-accent/60"
        :class="selected === m.id ? 'bg-accent' : 'hover:bg-accent/40'"
        @click="selected = m.id"
        @dblclick="!m.missing && emit('cue', m)"
        @keydown.enter.prevent="!m.missing && emit('cue', m)"
      >
        <div class="flex-1 min-w-0">
          <div class="flex items-center gap-1.5 min-w-0">
            <span class="text-sm truncate" :class="m.missing && 'text-muted-foreground line-through'" :title="m.path">{{ m.name }}</span>
            <span
              v-if="loadedId === m.id"
              class="shrink-0 rounded-sm bg-primary px-1 text-[9px] font-semibold uppercase tracking-wide text-primary-foreground"
            >
              On air
            </span>
          </div>
          <div class="flex items-center gap-2 text-[11px] text-muted-foreground">
            <span class="num">{{ m.info.duration_ms != null ? formatClock(m.info.duration_ms) : '—' }}</span>
            <span>{{ summary(m) }}</span>
            <span v-if="m.origin === 'recording'" class="uppercase tracking-wide text-[9px]">Recording</span>
          </div>
          <div v-if="m.missing" class="flex items-center gap-1 text-[11px] text-destructive">
            <FileWarning class="size-3" /> File no longer on disk
          </div>
        </div>
        <Tooltip>
          <TooltipTrigger as-child>
            <button
              class="size-6 grid place-items-center rounded text-muted-foreground opacity-0 group-hover:opacity-100 focus-visible:opacity-100 hover:text-foreground hover:bg-accent"
              :aria-label="`Remove ${m.name} from the library`"
              @click.stop="remove(m)"
            >
              <X class="size-3.5" />
            </button>
          </TooltipTrigger>
          <TooltipContent>Remove from library (the file stays on disk)</TooltipContent>
        </Tooltip>
      </li>
      <li v-if="!items.length" class="px-3 py-6 text-center text-xs text-muted-foreground">
        <template v-if="filter">Nothing matches.</template>
        <template v-else>
          No media yet. <button class="text-primary hover:underline" @click="browsing = true">Import a file</button>,
          or add one from a session in Recordings.
        </template>
      </li>
    </ul>

    <FileBrowseDialog v-if="browsing" :node-id="nodeId" @close="browsing = false" @select="importFile" />
  </div>
</template>
