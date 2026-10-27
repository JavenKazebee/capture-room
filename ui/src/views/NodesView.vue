<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { reloadAll } from '@/composables/useWebSocket'
import { notifyError } from '@/lib/notify'
import { useNodesStore } from '@/stores/nodes'
import { useStorageStore } from '@/stores/storage'
import { useChecksStore } from '@/stores/checks'
import type { NodeDto } from '@/types/generated/NodeDto'
import { Switch } from '@/components/ui/switch'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import PageHeader from '@/components/common/PageHeader.vue'
import ToolbarField from '@/components/common/ToolbarField.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import NodeCard from '@/components/nodes/NodeCard.vue'
import AddNodePopover from '@/components/nodes/AddNodePopover.vue'
import FoundNodes from '@/components/nodes/FoundNodes.vue'
import LoadError from '@/components/common/LoadError.vue'

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
      <AddNodePopover @added="refresh" />
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
    <LoadError :error="store.error" :retry="reloadAll" />
    <FoundNodes v-if="store.isController" @approved="refresh" />
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
