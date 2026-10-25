<script setup lang="ts">
import { computed, onMounted, ref, watch, type ComponentPublicInstance } from 'vue'
import { FetchError } from 'ofetch'
import { toast } from 'vue-sonner'
import { Plus } from '@lucide/vue'
import { errorMessage } from '@/composables/useApi'
import { reloadAll } from '@/composables/useWebSocket'
import { formatFingerprint } from '@/lib/format'
import { notifyError } from '@/lib/notify'
import { useNodesStore } from '@/stores/nodes'
import { useStorageStore } from '@/stores/storage'
import { useChecksStore } from '@/stores/checks'
import type { FoundNodeDto } from '@/types/generated/FoundNodeDto'
import type { NodeDto } from '@/types/generated/NodeDto'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Popover, PopoverAnchor, PopoverContent } from '@/components/ui/popover'
import { Switch } from '@/components/ui/switch'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import PageHeader from '@/components/common/PageHeader.vue'
import ToolbarField from '@/components/common/ToolbarField.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import NodeCard from '@/components/nodes/NodeCard.vue'

const store = useNodesStore()
const storage = useStorageStore()
const checks = useChecksStore()

// For each card's problem badge, and again as nodes become known (the list can arrive after a reload).
onMounted(() => checks.load())
watch(() => store.reachable.map((n) => n.id).join(), () => checks.load())

const peers = computed(() => store.nodes.filter((n) => !n.is_self))

async function refresh() {
  await reloadAll()
  await Promise.all([storage.load(), checks.load()])
}

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

/** The controller asks for a node's pairing code when the node doesn't use its token yet. */
function needsPairingCode(e: unknown) {
  return e instanceof FetchError && e.statusCode === 409
}

const addOpen = ref(false)
const addUrl = ref('')
const addCode = ref('')
/** Shown once the node turns out to need its pairing code. */
const addNeedsCode = ref(false)
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

watch(addUrl, () => {
  addNeedsCode.value = false
  addCode.value = ''
})

async function addNode() {
  const url = addUrl.value.trim()
  if (!url || adding.value) return
  addError.value = ''
  adding.value = true
  const before = new Set(store.nodes.map((n) => n.id))
  try {
    await store.add(url, addCode.value.trim())
    addUrl.value = ''
    addOpen.value = false
    const added = store.nodes.find((n) => !before.has(n.id))
    toast.success(added ? `Added ${added.name}` : 'Node already listed')
    await refresh()
  } catch (e) {
    if (needsPairingCode(e)) {
      addNeedsCode.value = true
      addError.value = ''
      return
    }
    // 502: the controller couldn't reach the address; its message is reqwest's, not for people.
    addError.value =
      e instanceof FetchError && e.statusCode === 502
        ? `Couldn't reach a Capture Room node at ${url}.`
        : errorMessage(e, 'Failed to add node')
  } finally {
    adding.value = false
  }
}

/** Found nodes with an approve or dismiss on its way. */
const deciding = ref(new Set<string>())
/** The found node asking for its pairing code, and the code typed so far. */
const codeFor = ref<string | null>(null)
const foundCode = ref('')

async function decide(node: FoundNodeDto, approve: boolean) {
  if (deciding.value.has(node.id)) return
  deciding.value.add(node.id)
  try {
    if (approve) {
      await store.approve(node.id, codeFor.value === node.id ? foundCode.value.trim() : undefined)
      codeFor.value = null
      toast.success(`Approved ${node.name}`)
      await refresh()
    } else {
      await store.dismiss(node.id)
    }
  } catch (e) {
    if (approve && needsPairingCode(e) && codeFor.value !== node.id) {
      codeFor.value = node.id
      foundCode.value = ''
    } else {
      notifyError(`Couldn't ${approve ? 'approve' : 'dismiss'} ${node.name}`, e)
    }
  } finally {
    deciding.value.delete(node.id)
  }
}

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
    <template #actions>
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
        <PopoverContent align="start" class="w-80" @interact-outside="onAddInteractOutside">
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
              <Button
                type="submit"
                size="sm"
                class="h-8 w-16"
                :disabled="adding || !addUrl.trim() || (addNeedsCode && !addCode.trim())"
              >
                {{ adding ? 'Adding…' : 'Add' }}
              </Button>
            </div>
            <div v-if="addNeedsCode" class="space-y-1">
              <Input
                v-model="addCode"
                placeholder="Pairing code"
                aria-label="Pairing code"
                class="h-8 font-mono text-xs uppercase"
                maxlength="12"
                spellcheck="false"
                autocomplete="off"
                :disabled="adding"
                autofocus
              />
              <p class="text-xs text-muted-foreground">
                It uses another access token. Enter the pairing code from its menu bar (or
                <code class="num">capture-room access-token show</code> on it).
              </p>
            </div>
            <p class="text-xs text-destructive min-h-4">{{ addError }}</p>
          </form>
        </PopoverContent>
      </Popover>
      <Tooltip>
        <TooltipTrigger as-child>
          <ToolbarField label="Controller">
            <Switch
              :model-value="store.isController"
              :disabled="toggling || !store.self"
              @update:model-value="onControllerToggle"
            />
          </ToolbarField>
        </TooltipTrigger>
        <TooltipContent class="max-w-64">
          Find other nodes on the network and control them from this machine. Applies immediately.
        </TooltipContent>
      </Tooltip>
    </template>
  </PageHeader>

  <div class="p-4 space-y-3">
    <section v-if="store.isController && store.found.length" class="rounded-lg border border-border bg-card">
      <header class="px-3 pt-3 pb-2">
        <h2 class="text-sm font-medium">Found on the network</h2>
        <p class="text-xs text-muted-foreground">
          Approve a node to control it from here with its pairing code, from its menu bar; it gets this
          controller's access token. Check its key matches the one under Settings › About on that machine first.
        </p>
      </header>
      <ul class="divide-y divide-border border-t border-border">
        <li v-for="f in store.found" :key="f.id" class="flex items-center gap-3 px-3 py-2">
          <div class="min-w-0 flex-1">
            <div class="text-sm truncate">
              {{ f.name }} <span class="text-xs text-muted-foreground num">· {{ f.url }} · v{{ f.version }}</span>
            </div>
            <div class="text-xs text-muted-foreground num break-all">Key {{ formatFingerprint(f.fingerprint) }}</div>
          </div>
          <Button
            size="sm"
            variant="ghost"
            class="h-7 text-xs"
            :disabled="deciding.has(f.id)"
            @click="decide(f, false)"
          >
            Dismiss
          </Button>
          <form v-if="codeFor === f.id" class="contents" @submit.prevent="decide(f, true)">
            <Input
              v-model="foundCode"
              placeholder="Pairing code"
              aria-label="Pairing code"
              class="h-7 w-32 font-mono text-xs uppercase"
              maxlength="12"
              spellcheck="false"
              autocomplete="off"
              :disabled="deciding.has(f.id)"
              autofocus
            />
            <Button type="submit" size="sm" class="h-7 text-xs" :disabled="deciding.has(f.id) || !foundCode.trim()">
              Approve
            </Button>
          </form>
          <Button v-else size="sm" class="h-7 text-xs" :disabled="deciding.has(f.id)" @click="decide(f, true)">
            Approve
          </Button>
        </li>
      </ul>
    </section>
    <div class="grid gap-3 grid-cols-[repeat(auto-fill,minmax(24rem,1fr))]">
      <NodeCard v-for="node in store.nodes" :key="node.id" :node="node" @remove="askRemove(node)" />
    </div>
    <p class="text-xs text-muted-foreground">
      <template v-if="store.isController">
        Nodes on this network appear above for you to approve with their pairing code. Approved nodes stay
        listed until removed.
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
    It stops appearing here and this controller stops trusting its key. Anything recording on it keeps
    recording. You can add it again by address, or approve it when it's found after a restart.
  </ConfirmDialog>
</template>
