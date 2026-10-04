<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { ArrowUp, Film, Folder, HardDrive, House, Loader2 } from '@lucide/vue'
import { nodeApi, errorMessage } from '@/composables/useApi'
import { useSourcesStore } from '@/stores/sources'
import { formatBytes } from '@/lib/format'
import type { DirListingDto } from '@/types/generated/DirListingDto'
import type { DirEntryDto } from '@/types/generated/DirEntryDto'
import type { StorageVolumeDto } from '@/types/generated/StorageVolumeDto'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'

/**
 * Pick a media file on a node's disk. Lists folders and media files only.
 * Mount it with `v-if`; it starts in `start`'s folder, or the node's home.
 */
const props = defineProps<{ nodeId: string; start?: string }>()
const emit = defineEmits<{ close: []; select: [path: string] }>()

const store = useSourcesStore()
const listing = ref<DirListingDto | null>(null)
const volumes = ref<StorageVolumeDto[]>([])
const selected = ref<string | null>(null)
const loading = ref(false)
const error = ref<string | null>(null)

async function open(path?: string) {
  loading.value = true
  error.value = null
  try {
    listing.value = await store.listFiles(props.nodeId, path)
    selected.value = null
  } catch (e) {
    error.value = errorMessage(e, 'Could not read the folder.')
  } finally {
    loading.value = false
  }
}

function activate(entry: DirEntryDto) {
  if (entry.is_dir) open(entry.path)
  else selected.value = entry.path
}

function choose(path = selected.value) {
  if (!path) return
  emit('select', path)
  emit('close')
}

/** The folder holding `path`, if it looks like a file path. */
function folderOf(path?: string) {
  const i = path ? Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\')) : -1
  return i > 0 ? path!.slice(0, i) : undefined
}

onMounted(async () => {
  nodeApi(props.nodeId)<StorageVolumeDto[]>('/storage')
    .then((v) => (volumes.value = v))
    .catch(() => {})
  await open(folderOf(props.start))
  // A stale start folder: fall back to home.
  if (!listing.value && props.start) await open()
})
</script>

<template>
  <Dialog :open="true" @update:open="(v) => !v && emit('close')">
    <DialogContent class="sm:max-w-xl gap-3">
      <DialogHeader>
        <DialogTitle>Choose a media file</DialogTitle>
        <DialogDescription>Files on the node's disk. Only folders and video files are listed.</DialogDescription>
      </DialogHeader>

      <div class="flex items-center gap-1">
        <Button
          variant="outline"
          size="icon"
          title="Up one folder"
          :disabled="!listing?.parent || loading"
          @click="open(listing!.parent!)"
        >
          <ArrowUp />
        </Button>
        <Button variant="outline" size="icon" title="Home folder" :disabled="loading" @click="open()">
          <House />
        </Button>
        <div class="min-w-0 flex-1 truncate rounded-md border border-border bg-muted/40 px-2 h-7 leading-7 font-mono text-[0.6875rem]" :title="listing?.path">
          {{ listing?.path ?? '…' }}
        </div>
      </div>

      <div v-if="volumes.length > 1" class="flex flex-wrap gap-1">
        <Button
          v-for="v in volumes"
          :key="v.mount_point"
          variant="ghost"
          size="sm"
          class="text-muted-foreground"
          :title="v.mount_point"
          @click="open(v.mount_point)"
        >
          <HardDrive /> {{ v.name || v.mount_point }}
        </Button>
      </div>

      <div class="h-72 overflow-y-auto rounded-md border border-border">
        <div v-if="loading" class="h-full grid place-items-center text-muted-foreground">
          <Loader2 class="size-4 animate-spin" />
        </div>
        <p v-else-if="error" class="p-3 text-destructive">{{ error }}</p>
        <p v-else-if="!listing?.entries.length" class="p-3 text-muted-foreground">No folders or media files here.</p>
        <ul v-else>
          <li v-for="entry in listing.entries" :key="entry.path">
            <button
              type="button"
              class="w-full flex items-center gap-2 px-2 py-1 text-left hover:bg-accent"
              :class="selected === entry.path && 'bg-primary/15 hover:bg-primary/20'"
              @click="activate(entry)"
              @dblclick="!entry.is_dir && choose(entry.path)"
            >
              <Folder v-if="entry.is_dir" class="size-3.5 shrink-0 text-muted-foreground" />
              <Film v-else class="size-3.5 shrink-0 text-primary" />
              <span class="min-w-0 flex-1 truncate">{{ entry.name }}</span>
              <span v-if="entry.size != null" class="num text-muted-foreground">{{ formatBytes(entry.size) }}</span>
            </button>
          </li>
        </ul>
      </div>

      <DialogFooter class="items-center">
        <p class="mr-auto min-w-0 truncate font-mono text-[0.6875rem] text-muted-foreground" :title="selected ?? ''">
          {{ selected ?? 'Select a file' }}
        </p>
        <Button variant="outline" @click="emit('close')">Cancel</Button>
        <Button :disabled="!selected" @click="choose()">Choose</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
