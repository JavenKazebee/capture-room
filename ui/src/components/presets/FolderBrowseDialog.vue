<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { Archive, ArrowUp, Folder, FolderPlus, HardDrive, House, Loader2, MapPin } from '@lucide/vue'
import { errorMessage } from '@/composables/useApi'
import { useNodesStore } from '@/stores/nodes'
import { useStorageStore } from '@/stores/storage'
import type { FolderDto } from '@/types/generated/FolderDto'
import type { FolderListingDto } from '@/types/generated/FolderListingDto'
import OptionSelect from '@/components/OptionSelect.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'

/**
 * Pick a folder on one node's disk. Mount with `v-if`; starts in `start` (a plain path, `~` for
 * home) on this node, or at the node's places when that folder isn't there.
 */
const props = defineProps<{ start?: string }>()
const emit = defineEmits<{ close: []; select: [path: string] }>()

const nodes = useNodesStore()
const storage = useStorageStore()

/** Presets aren't tied to a node, so browsing starts on this one (the controller, on a cluster). */
const nodeId = ref(nodes.nodes.find((n) => n.is_self)?.id ?? nodes.reachable[0]?.id ?? '')
const nodeOptions = computed(() => nodes.reachable.map((n) => ({ value: n.id, label: nodes.labelOf(n.id) })))

const listing = ref<FolderListingDto | null>(null)
const loading = ref(false)
const error = ref<string | null>(null)
/** The new folder's name while it's being typed; `null` when not adding one. */
const newName = ref<string | null>(null)
const creating = ref(false)
const newInput = ref<{ $el: HTMLInputElement } | null>(null)

/** Counts `open` calls: only the latest one's listing is shown. */
let opens = 0

async function open(path?: string) {
  const mine = ++opens
  loading.value = true
  error.value = null
  newName.value = null
  try {
    const next = await storage.listFolders(nodeId.value, path)
    if (mine !== opens) return
    listing.value = next
  } catch (e) {
    if (mine === opens) error.value = errorMessage(e, 'Could not read the folder.')
  } finally {
    if (mine === opens) loading.value = false
  }
}

const KIND_ICONS = { folder: Folder, home: House, storage: Archive, volume: HardDrive }

function startNew() {
  newName.value = ''
  nextTick(() => newInput.value?.$el.focus())
}

async function create() {
  const parent = listing.value?.path
  const name = newName.value?.trim()
  if (!parent || !name || creating.value) return
  creating.value = true
  error.value = null
  try {
    const made = await storage.createFolder(nodeId.value, parent, name)
    await open(made.path)
  } catch (e) {
    error.value = errorMessage(e, 'Could not create the folder.')
  } finally {
    creating.value = false
  }
}

function choose(path = listing.value?.path) {
  if (!path) return
  emit('select', path)
  emit('close')
}

function enter(folder: FolderDto) {
  open(folder.path)
}

watch(nodeId, () => open())

onMounted(async () => {
  await open(props.start)
  // Not on this node (yet): start at its places.
  if (error.value && props.start) await open()
})
</script>

<template>
  <Dialog :open="true" @update:open="(v) => !v && emit('close')">
    <DialogContent class="sm:max-w-xl gap-3">
      <DialogHeader>
        <DialogTitle>Choose a folder</DialogTitle>
        <DialogDescription>Folders on one node's disk. Other nodes need the same path for this preset to record there.</DialogDescription>
      </DialogHeader>

      <OptionSelect v-if="nodeOptions.length > 1" v-model="nodeId" :options="nodeOptions" aria-label="Node" />

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
        <Button variant="outline" size="icon" title="Places" :disabled="loading || !listing?.path" @click="open()">
          <MapPin />
        </Button>
        <div class="min-w-0 flex-1 truncate rounded-md border border-border bg-muted/40 px-2 h-7 leading-7 font-mono text-[0.6875rem]" :title="listing?.path ?? ''">
          {{ listing ? (listing.path ?? 'Places') : '…' }}
        </div>
        <Button variant="outline" size="icon" title="New folder" :disabled="loading || !listing?.path" @click="startNew">
          <FolderPlus />
        </Button>
      </div>

      <form v-if="newName !== null" class="flex items-center gap-1" @submit.prevent="create">
        <Input
          ref="newInput"
          v-model="newName"
          placeholder="New folder name"
          aria-label="New folder name"
          class="flex-1"
          @keydown.esc.stop.prevent="newName = null"
        />
        <Button type="button" variant="ghost" @click="newName = null">Cancel</Button>
        <Button type="submit" :disabled="!newName.trim() || creating">Create</Button>
      </form>

      <div class="h-72 overflow-y-auto rounded-md border border-border">
        <div v-if="loading" class="h-full grid place-items-center text-muted-foreground">
          <Loader2 class="size-4 animate-spin" />
        </div>
        <p v-else-if="error && !listing" class="p-3 text-destructive">{{ error }}</p>
        <p v-else-if="!listing?.folders.length" class="p-3 text-muted-foreground">No folders here.</p>
        <ul v-else>
          <li v-for="folder in listing.folders" :key="folder.path">
            <button
              type="button"
              class="w-full flex items-center gap-2 px-2 py-1 text-left hover:bg-accent"
              :title="folder.path"
              @click="enter(folder)"
            >
              <component :is="KIND_ICONS[folder.kind]" class="size-3.5 shrink-0 text-muted-foreground" />
              <span class="min-w-0 flex-1 truncate">{{ folder.name }}</span>
              <span v-if="folder.kind !== 'folder'" class="min-w-0 truncate font-mono text-[0.6875rem] text-muted-foreground">
                {{ folder.path }}
              </span>
            </button>
          </li>
        </ul>
      </div>
      <p v-if="error && listing" class="text-[11px] text-destructive">{{ error }}</p>

      <DialogFooter class="items-center">
        <p class="mr-auto min-w-0 truncate font-mono text-[0.6875rem] text-muted-foreground" :title="listing?.path ?? ''">
          {{ listing?.path ?? 'Open a folder' }}
        </p>
        <Button variant="outline" @click="emit('close')">Cancel</Button>
        <Button :disabled="!listing?.path || loading" @click="choose()">Choose this folder</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
