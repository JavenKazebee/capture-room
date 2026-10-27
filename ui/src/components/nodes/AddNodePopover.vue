<script setup lang="ts">
import { ref, watch, type ComponentPublicInstance } from 'vue'
import { FetchError } from 'ofetch'
import { toast } from 'vue-sonner'
import { Plus } from '@lucide/vue'
import { errorMessage } from '@/composables/useApi'
import { needsPairingCode, useNodesStore } from '@/stores/nodes'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Popover, PopoverAnchor, PopoverContent } from '@/components/ui/popover'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'

/** "Add node" by address, asking for the node's pairing code when it turns out to need one. */
const emit = defineEmits<{ added: [] }>()

const store = useNodesStore()

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
    emit('added')
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
</script>

<template>
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
</template>
