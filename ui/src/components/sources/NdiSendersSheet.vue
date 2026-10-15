<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { nodeApi } from '@/composables/useApi'
import { useNodesStore } from '@/stores/nodes'
import { useSourcesStore } from '@/stores/sources'
import { notifyError } from '@/lib/notify'
import type { NdiConnect } from '@/types/generated/NdiConnect'
import type { NdiSenderDto } from '@/types/generated/NdiSenderDto'
import type { NdiSubscriptionsDto } from '@/types/generated/NdiSubscriptionsDto'
import type { NdiSubscriptionsInput } from '@/types/generated/NdiSubscriptionsInput'
import { Switch } from '@/components/ui/switch'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import OptionSelect from '@/components/OptionSelect.vue'

/**
 * Which NDI senders each node connects to: every one it finds (the default),
 * or only the chosen ones, so a node doesn't pull feeds it never uses. A
 * sender a recording or replay buffer uses stays connected either way, and a
 * node never connects to its own outputs. Changes apply at once.
 */
const emit = defineEmits<{ close: [] }>()

const nodes = useNodesStore()
const sources = useSourcesStore()

const CONNECT_OPTIONS: { value: NdiConnect; label: string }[] = [
  { value: 'all', label: 'Every sender it finds' },
  { value: 'chosen', label: 'Only the chosen ones' },
]

/** Each reachable node's subscriptions, by node id. */
const subs = ref(new Map<string, NdiSubscriptionsDto>())
const errors = ref(new Map<string, string>())
const busy = ref<string | null>(null)

async function load(nodeId: string) {
  try {
    subs.value.set(nodeId, await nodeApi(nodeId)<NdiSubscriptionsDto>('/ndi'))
    errors.value.delete(nodeId)
  } catch (e) {
    errors.value.set(nodeId, e instanceof Error ? e.message : String(e))
  }
}

async function save(nodeId: string, input: NdiSubscriptionsInput) {
  busy.value = nodeId
  try {
    subs.value.set(nodeId, await nodeApi(nodeId)<NdiSubscriptionsDto>('/ndi', { method: 'PUT', body: input }))
    await sources.refreshNode(nodeId)
  } catch (e) {
    notifyError(`Changing NDI senders on ${nodes.nameOf(nodeId)} failed`, e, nodeId)
  } finally {
    busy.value = null
  }
}

function setConnect(nodeId: string, connect: NdiConnect) {
  const s = subs.value.get(nodeId)
  if (!s || s.connect === connect) return
  // Switching to chosen starts from what's connected now, so nothing drops.
  const chosen =
    connect === 'chosen' && !s.chosen.length ? s.senders.filter((x) => x.connected).map((x) => x.ndi_name) : s.chosen
  save(nodeId, { connect, chosen })
}

function choose(nodeId: string, sender: NdiSenderDto, on: boolean) {
  const s = subs.value.get(nodeId)
  if (!s) return
  const chosen = s.chosen.filter((n) => n !== sender.ndi_name)
  if (on) chosen.push(sender.ndi_name)
  save(nodeId, { connect: s.connect, chosen })
}

function isOn(s: NdiSubscriptionsDto, x: NdiSenderDto) {
  if (x.own) return false
  return s.connect === 'all' || x.in_use || s.chosen.includes(x.ndi_name)
}

function note(s: NdiSubscriptionsDto, x: NdiSenderDto) {
  if (x.own) return "This node's output"
  if (x.in_use && s.connect === 'chosen' && !s.chosen.includes(x.ndi_name)) return 'In use'
  return ''
}

onMounted(() => Promise.all(nodes.reachable.map((n) => load(n.id))))
</script>

<template>
  <Sheet :open="true" @update:open="(v) => !v && emit('close')">
    <SheetContent class="flex flex-col gap-0 p-0 w-full sm:max-w-lg">
      <SheetHeader class="border-b border-border">
        <SheetTitle>NDI senders</SheetTitle>
        <SheetDescription>
          Each node connects to every NDI sender it finds. A node that only needs some of them, like a replay
          node, can connect to just those. A sender a recording or replay buffer uses stays connected.
        </SheetDescription>
      </SheetHeader>

      <div class="flex-1 overflow-y-auto divide-y divide-border">
        <section v-for="n in nodes.reachable" :key="n.id" class="p-4 flex flex-col gap-3">
          <div class="flex items-center gap-3">
            <span class="text-sm font-medium truncate flex-1">{{ nodes.nameOf(n.id) }}</span>
            <OptionSelect
              v-if="subs.get(n.id)"
              :model-value="subs.get(n.id)!.connect"
              :options="CONNECT_OPTIONS"
              :disabled="busy === n.id"
              class="h-8 w-52"
              :aria-label="`Which NDI senders ${nodes.nameOf(n.id)} connects to`"
              @update:model-value="(c) => setConnect(n.id, c)"
            />
          </div>
          <p v-if="errors.get(n.id)" class="text-xs text-destructive">{{ errors.get(n.id) }}</p>
          <template v-else-if="subs.get(n.id)">
            <p v-if="!subs.get(n.id)!.senders.length" class="text-xs text-muted-foreground">No NDI senders found.</p>
            <label
              v-for="x in subs.get(n.id)!.senders"
              :key="x.ndi_name"
              class="flex items-center gap-2 min-w-0"
            >
              <Switch
                :model-value="isOn(subs.get(n.id)!, x)"
                :disabled="busy === n.id || x.own || x.in_use || subs.get(n.id)!.connect === 'all'"
                :aria-label="`Connect ${nodes.nameOf(n.id)} to ${x.ndi_name}`"
                @update:model-value="(on: boolean) => choose(n.id, x, on)"
              />
              <span class="text-sm truncate" :class="x.own && 'text-muted-foreground'">{{ x.ndi_name }}</span>
              <span class="flex-1" />
              <span class="text-xs text-muted-foreground shrink-0">{{ note(subs.get(n.id)!, x) }}</span>
            </label>
          </template>
          <p v-else class="text-xs text-muted-foreground">Loading…</p>
        </section>
      </div>
    </SheetContent>
  </Sheet>
</template>
