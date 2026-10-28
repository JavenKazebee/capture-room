<script setup lang="ts">
import { computed, onScopeDispose, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import { Settings2, ShieldQuestion } from '@lucide/vue'
import { notifyError } from '@/lib/notify'
import { browserIsOn, useChecksStore } from '@/stores/checks'
import { useNodesStore } from '@/stores/nodes'
import { Button } from '@/components/ui/button'

/** A permission the node lacks, which keeps some of its devices off the list, and a way to ask for it. */
const props = defineProps<{ nodeId: string; permission: 'microphone' | 'screen_recording' }>()
const emit = defineEmits<{ answered: [] }>()

const store = useChecksStore()
const nodes = useNodesStore()

watch(
  () => props.nodeId,
  (id) => {
    if (id && !store.byNode.has(id)) void store.loadForNode(id)
  },
  { immediate: true },
)

const check = computed(() => store.byNode.get(props.nodeId)?.checks.find((c) => c.id === props.permission))
const missing = computed(() => check.value?.status === 'missing' || check.value?.status === 'not_asked')
const local = computed(() => {
  const node = nodes.nodes.find((n) => n.id === props.nodeId)
  return !!node && browserIsOn(node)
})
const what = computed(() =>
  props.permission === 'microphone'
    ? { access: 'microphone access', devices: 'audio devices' }
    : { access: 'screen recording', devices: 'screens' },
)

const asking = ref(false)
const asked = ref(false)
let disposed = false
onScopeDispose(() => (disposed = true))

async function ask() {
  asking.value = true
  try {
    await store.ask(props.nodeId, props.permission, () => disposed)
    asked.value = true
    emit('answered')
  } catch (e) {
    notifyError(`Couldn't ask macOS for ${what.value.access}`, e, props.nodeId)
  } finally {
    asking.value = false
  }
}
</script>

<template>
  <div v-if="missing" class="flex flex-col gap-2 rounded-md bg-muted/50 px-3 py-2">
    <p class="text-xs">
      The node's {{ what.devices }} aren't listed: macOS hasn't allowed {{ what.access }}.
      <template v-if="asked && permission === 'screen_recording'">
        Once it's allowed, restart the node to list them.
      </template>
      <template v-else-if="!local">Allow it in Setup › Checks, opened on the node itself.</template>
    </p>
    <div v-if="local" class="flex flex-wrap items-center gap-1.5">
      <Button
        v-if="check?.fix?.can_request"
        type="button"
        size="sm"
        class="h-7 gap-1.5 text-xs"
        :disabled="asking"
        @click="ask"
      >
        <ShieldQuestion class="size-3.5" />
        {{ asking ? 'Waiting for an answer…' : 'Request access' }}
      </Button>
      <Button v-if="check?.fix?.settings_url" as-child variant="outline" size="sm" class="h-7 gap-1.5 text-xs">
        <a :href="check.fix.settings_url"><Settings2 class="size-3.5" /> Open System Settings</a>
      </Button>
    </div>
    <RouterLink v-else :to="{ name: 'checks', hash: `#node-${nodeId}` }" class="text-xs underline self-start">
      Go to Setup › Checks
    </RouterLink>
  </div>
</template>
