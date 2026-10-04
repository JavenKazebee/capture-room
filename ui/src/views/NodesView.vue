<script setup lang="ts">
import { computed, ref, type ComponentPublicInstance } from 'vue'
import { FetchError } from 'ofetch'
import { toast } from 'vue-sonner'
import { Plus } from '@lucide/vue'
import { errorMessage } from '@/composables/useApi'
import { reloadAll } from '@/composables/useWebSocket'
import { notifyError } from '@/lib/notify'
import { useNodesStore } from '@/stores/nodes'
import { useStorageStore } from '@/stores/storage'
import type { NodeDto } from '@/types/generated/NodeDto'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Popover, PopoverAnchor, PopoverContent } from '@/components/ui/popover'
import { Switch } from '@/components/ui/switch'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import PageHeader from '@/components/common/PageHeader.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import NodeCard from '@/components/nodes/NodeCard.vue'

const store = useNodesStore()
const storage = useStorageStore()

const peers = computed(() => store.nodes.filter((n) => !n.is_self))

async function refresh() {
  await reloadAll()
  await storage.load()
}

// ── Controller ────────────────────────────────────────────────────────────────

const toggling = ref(false)
const confirmDisable = ref(false)
/** Peer names when the confirm opened, so its text doesn't change as they disappear. */
const disabling = ref<string[]>([])

function onControllerToggle(on: boolean) {
  if (!on && peers.value.length) {
    disabling.value = peers.value.map((n) => n.name)
    confirmDisable.value = true
  } else {
    setController(on)
  }
}

async function setController(on: boolean) {
  if (toggling.value) return
  toggling.value = true
  try {
    await store.setController(on)
    await refresh()
  } catch (e) {
    notifyError(`Couldn't turn the controller ${on ? 'on' : 'off'}`, e)
  } finally {
    toggling.value = false
  }
}

// ── Add by address ────────────────────────────────────────────────────────────

const addOpen = ref(false)
const addUrl = ref('')
const addError = ref('')
const adding = ref(false)

const addAnchor = ref<ComponentPublicInstance | null>(null)

/** Clicks on the button toggle the popover themselves; don't let them count as "outside". */
function onAddInteractOutside(e: Event) {
  if (addAnchor.value?.$el.contains(e.target as Node)) e.preventDefault()
}

function onAddOpen(open: boolean) {
  addOpen.value = open
  if (open) addError.value = ''
}

async function addNode() {
  const url = addUrl.value.trim()
  if (!url || adding.value) return
  addError.value = ''
  adding.value = true
  const before = new Set(store.nodes.map((n) => n.id))
  try {
    await store.add(url)
    addUrl.value = ''
    addOpen.value = false
    const added = store.nodes.find((n) => !before.has(n.id))
    toast.success(added ? `Added ${added.name}` : 'Node already listed')
    await refresh()
  } catch (e) {
    // 502: the controller couldn't reach the address; its message is reqwest's, not for people.
    addError.value =
      e instanceof FetchError && e.statusCode === 502
        ? `Couldn't reach a Capture Room node at ${url}.`
        : errorMessage(e, 'Failed to add node')
  } finally {
    adding.value = false
  }
}

// ── Remove ────────────────────────────────────────────────────────────────────

// Kept after the dialog closes so its title doesn't change mid-animation.
const removing = ref<NodeDto | null>(null)
const confirmRemove = ref(false)

function askRemove(node: NodeDto) {
  removing.value = node
  confirmRemove.value = true
}

async function removeNode() {
  const node = removing.value
  if (!node) return
  try {
    await store.remove(node.id)
    await refresh()
  } catch (e) {
    notifyError(`Couldn't remove ${node.name}`, e, node.id)
  }
}
</script>

<template>
  <PageHeader title="Nodes" :count="store.nodes.length">
    <Tooltip>
      <TooltipTrigger as-child>
        <label class="flex items-center gap-1.5 text-xs text-muted-foreground">
          <Switch
            :model-value="store.isController"
            :disabled="toggling || !store.self"
            @update:model-value="onControllerToggle"
          />
          Controller
        </label>
      </TooltipTrigger>
      <TooltipContent class="max-w-64">
        Find other nodes on the network and control them from this machine. Applies immediately.
      </TooltipContent>
    </Tooltip>
    <div class="w-px h-5 bg-border mx-1" />
    <Popover :open="addOpen" @update:open="onAddOpen">
      <!-- Anchored on a wrapper: a trigger or anchor can't nest inside the tooltip's trigger. -->
      <PopoverAnchor ref="addAnchor" class="inline-flex">
        <Tooltip :disabled="store.isController">
          <TooltipTrigger as-child>
            <!-- A disabled button gets no pointer events; the span keeps the tooltip working. -->
            <span tabindex="-1">
              <Button
                size="sm"
                class="h-7 gap-1.5 text-xs"
                :disabled="!store.isController"
                aria-haspopup="dialog"
                :aria-expanded="addOpen"
                @click="onAddOpen(!addOpen)"
              >
                <Plus class="size-3.5" /> Add node
              </Button>
            </span>
          </TooltipTrigger>
          <TooltipContent>Turn on Controller to add nodes.</TooltipContent>
        </Tooltip>
      </PopoverAnchor>
      <PopoverContent align="end" class="w-80" @interact-outside="onAddInteractOutside">
        <form class="space-y-2" @submit.prevent="addNode">
          <div class="text-sm font-medium">Add node by address</div>
          <p class="text-xs text-muted-foreground">
            For a node on another subnet. Nodes on this network are found automatically.
          </p>
          <div class="flex gap-2">
            <Input
              v-model="addUrl"
              placeholder="192.168.1.20:7700"
              class="h-8 num text-xs"
              :disabled="adding"
              :aria-invalid="!!addError || undefined"
            />
            <Button type="submit" size="sm" class="h-8 w-16" :disabled="adding || !addUrl.trim()">
              {{ adding ? 'Adding…' : 'Add' }}
            </Button>
          </div>
          <p class="text-xs text-destructive min-h-4">{{ addError }}</p>
        </form>
      </PopoverContent>
    </Popover>
  </PageHeader>

  <div class="p-4 space-y-3">
    <div class="grid gap-3 grid-cols-[repeat(auto-fill,minmax(24rem,1fr))]">
      <NodeCard v-for="node in store.nodes" :key="node.id" :node="node" @remove="askRemove(node)" />
    </div>
    <p class="text-xs text-muted-foreground">
      <template v-if="store.isController">
        Nodes on this network appear automatically. Discovered nodes drop off about 15 seconds after
        they go offline; nodes added by address stay listed until removed.
      </template>
      <template v-else>
        This machine records its own sources. Turn on Controller to find other nodes on the network and
        control them from here.
      </template>
    </p>
  </div>

  <ConfirmDialog
    v-model:open="confirmDisable"
    title="Turn off controller?"
    confirm-label="Turn off"
    @confirm="setController(false)"
  >
    <template v-if="disabling.length === 1">{{ disabling[0] }} will disappear from this UI. Anything recording on it keeps recording.</template>
    <template v-else>{{ disabling.length }} other nodes will disappear from this UI. Anything recording on them keeps recording.</template>
  </ConfirmDialog>

  <ConfirmDialog
    v-model:open="confirmRemove"
    :title="`Remove ${removing?.name ?? 'node'}?`"
    confirm-label="Remove"
    @confirm="removeNode"
  >
    It stops appearing here and won't be restored on restart. Anything recording on it keeps
    recording. You can add it again by address.
  </ConfirmDialog>
</template>
