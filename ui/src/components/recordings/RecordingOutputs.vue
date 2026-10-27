<script setup lang="ts">
import { computed, ref } from 'vue'
import { useClipboard } from '@vueuse/core'
import { useRouter } from 'vue-router'
import { toast } from 'vue-sonner'
import { Download, ListPlus, Play, Share2, Trash2 } from '@lucide/vue'
import { recordingFileUrl } from '@/composables/useApi'
import { fileName } from '@/lib/format'
import { notifyError } from '@/lib/notify'
import { shareLink } from '@/lib/share'
import { useNodesStore } from '@/stores/nodes'
import { usePlayoutStore } from '@/stores/playout'
import { useRecordingsStore, type RecordingSession } from '@/stores/recordings'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import CopyButton from '@/components/common/CopyButton.vue'

/** A recording's outputs and their files, each with its actions. */
const props = defineProps<{
  session: RecordingSession
  outputs: { name: string; format: string | null; files: string[]; dropped: number }[]
  canPlay: (output: number) => boolean
  isPlaying: (output: number, file: number) => boolean
}>()
const emit = defineEmits<{ play: [output: number, file: number] }>()

const recordings = useRecordingsStore()
const nodes = useNodesStore()
const playout = usePlayoutStore()
const router = useRouter()
const clipboard = useClipboard({ legacy: true })

const s = computed(() => props.session)
const live = computed(() => s.value.status === 'active')

/** Adds a file to its node's media library, for playing out from Playback. */
async function addToMedia(path: string) {
  try {
    const item = await playout.addMedia(s.value.node_id, path, s.value.id)
    toast.success(`Added ${item.name} to the media library`, {
      action: { label: 'Open Playback', onClick: () => router.push('/playback') },
    })
  } catch (e) {
    notifyError('Could not add the file to the media library', e, s.value.node_id)
  }
}

async function share(output: number, file: number, path: string) {
  const url = new URL(recordingFileUrl(s.value.node_id, s.value.id, output, file, true), window.location.origin).href
  const result = await shareLink(fileName(path), url, async (text) => {
    await clipboard.copy(text)
    return clipboard.copied.value
  })
  if (result === 'copied') toast.success('Download link copied', { description: 'Anyone who can reach this Capture Room can open it.' })
  else if (result === 'failed') toast.error('Could not copy the link', { description: url })
}

/** The single file awaiting confirmation to delete. */
const deleting = ref<{ output: number; file: number; path: string } | null>(null)
const confirmDeleteFile = computed({
  get: () => deleting.value !== null,
  set: (open) => {
    if (!open) deleting.value = null
  },
})

async function deleteFile() {
  const d = deleting.value
  if (!d) return
  try {
    await recordings.deleteFile(s.value.node_id, s.value.id, d.output, d.file, d.path)
    toast.success(`Deleted ${fileName(d.path)}`)
  } catch (e) {
    notifyError(`Could not delete ${fileName(d.path)}`, e, s.value.node_id)
  }
}
</script>

<template>
  <section class="space-y-1.5">
    <h3 class="section-title">Outputs</h3>
  <div v-for="(o, i) in outputs" :key="i" class="rounded-md border border-border">
    <div class="flex items-baseline gap-2 px-2.5 py-1.5 border-b border-border/60">
      <span class="text-xs font-medium">{{ o.name }}</span>
      <span v-if="o.format" class="text-[11px] text-muted-foreground truncate">{{ o.format }}</span>
      <div class="flex-1" />
      <span v-if="o.dropped" class="num text-[11px] text-warning shrink-0" title="Video frames dropped because this output's encoder couldn't keep up">
        {{ o.dropped.toLocaleString() }} dropped
      </span>
    </div>
    <ul class="divide-y divide-border/60">
      <li
        v-for="(f, j) in o.files"
        :key="f"
        class="flex items-center gap-1 px-2.5 py-1 text-xs"
        :class="[
          canPlay(i) && 'cursor-pointer hover:bg-accent/40',
          isPlaying(i, j) && 'bg-accent/60',
        ]"
        :role="canPlay(i) ? 'button' : undefined"
        :tabindex="canPlay(i) ? 0 : undefined"
        :aria-label="canPlay(i) ? `Play ${fileName(f)}` : undefined"
        :aria-current="isPlaying(i, j) || undefined"
        :title="!live && !canPlay(i) ? 'This format can\'t play in a browser. Download it to watch.' : undefined"
        @click="emit('play', i, j)"
        @keydown.enter.self.prevent="emit('play', i, j)"
        @keydown.space.self.prevent="emit('play', i, j)"
      >
        <span class="w-3.5 shrink-0 grid place-items-center">
          <Play v-if="isPlaying(i, j)" class="size-3 text-primary fill-current" />
          <Play v-else-if="canPlay(i)" class="size-3 text-muted-foreground/50" />
        </span>
        <span
          class="num truncate flex-1"
          :class="isPlaying(i, j) ? 'text-foreground' : canPlay(i) || live ? 'text-muted-foreground' : 'text-muted-foreground/60'"
          :title="f"
        >
          {{ fileName(f) }}
        </span>
        <span v-if="isPlaying(i, j)" class="text-[10px] font-medium uppercase tracking-wider text-primary shrink-0 mr-1">Playing</span>
        <CopyButton :value="f" size="md" :label="`Copy path on ${nodes.labelOf(s.node_id)}`" />
        <Tooltip v-if="!live">
          <TooltipTrigger as-child>
            <button class="icon-btn" :aria-label="`Add ${fileName(f)} to the media library`" @click.stop="addToMedia(f)"><ListPlus class="size-3.5" /></button>
          </TooltipTrigger>
          <TooltipContent>Add to media library (for Playback)</TooltipContent>
        </Tooltip>
        <Tooltip v-if="!live">
          <TooltipTrigger as-child>
            <button class="icon-btn" :aria-label="`Share ${fileName(f)}`" @click.stop="share(i, j, f)"><Share2 class="size-3.5" /></button>
          </TooltipTrigger>
          <TooltipContent>Share download link</TooltipContent>
        </Tooltip>
        <Tooltip v-if="!live">
          <TooltipTrigger as-child>
            <a
              class="icon-btn"
              :href="recordingFileUrl(s.node_id, s.id, i, j, true)"
              :aria-label="`Download ${fileName(f)}`"
              download
              @click.stop
            >
              <Download class="size-3.5" />
            </a>
          </TooltipTrigger>
          <TooltipContent>Download</TooltipContent>
        </Tooltip>
        <Tooltip v-if="!live">
          <TooltipTrigger as-child>
            <button
              class="icon-btn hover:text-destructive!"
              :aria-label="`Delete ${fileName(f)}`"
              @click.stop="deleting = { output: i, file: j, path: f }"
            >
              <Trash2 class="size-3.5" />
            </button>
          </TooltipTrigger>
          <TooltipContent>Delete file</TooltipContent>
        </Tooltip>
      </li>
      <li v-if="!o.files.length" class="px-2.5 py-1 text-xs italic text-muted-foreground/60">No files</li>
    </ul>
  </div>

    <ConfirmDialog
      v-model:open="confirmDeleteFile"
      :title="`Delete ${deleting ? fileName(deleting.path) : ''}?`"
      :description="`The file will be permanently deleted from ${nodes.labelOf(s.node_id)}'s disk. This can't be undone. The session stays in history.`"
      confirm-label="Delete file"
      @confirm="deleteFile"
    />
  </section>
</template>

<style scoped>
@reference "@/style.css";
.icon-btn {
  @apply size-6 shrink-0 grid place-items-center rounded text-muted-foreground hover:text-foreground hover:bg-accent;
}
.section-title {
  @apply text-[11px] font-semibold uppercase tracking-wider text-muted-foreground;
}
</style>
