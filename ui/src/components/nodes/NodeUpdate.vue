<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { Download, LoaderCircle, TriangleAlert } from '@lucide/vue'
import { nodeApi } from '@/composables/useApi'
import { notifyError } from '@/lib/notify'
import type { NodeDto } from '@/types/generated/NodeDto'
import type { UpdateStatusDto } from '@/types/generated/UpdateStatusDto'
import { Button } from '@/components/ui/button'

/**
 * A node's software update, in its card's footer: Update to the newest release when one is
 * out, its progress while it installs. Only the installed app updates itself, so nodes run
 * from a shell show nothing. Polled: slowly while idle, every second while updating (the
 * node goes away while it restarts, and comes back on the new version).
 */
const props = defineProps<{ node: NodeDto }>()

const IDLE_POLL_MS = 60_000
const BUSY_POLL_MS = 1_000

const status = ref<UpdateStatusDto | null>(null)
const starting = ref(false)
let timer: ReturnType<typeof setTimeout> | undefined

const updating = computed(() => starting.value || (status.value != null && status.value.phase !== 'idle' && status.value.phase !== 'checking'))
const shown = computed(() => status.value != null && !status.value.unsupported && (status.value.available || updating.value || !!status.value.error))

const progress = computed(() => {
  const s = status.value
  if (!s) return ''
  switch (s.phase) {
    case 'downloading':
      return s.percent != null ? `Downloading ${s.percent}%` : 'Downloading'
    case 'installing':
      return 'Installing'
    case 'restarting':
      return 'Restarting'
    default:
      return 'Starting'
  }
})

async function poll() {
  clearTimeout(timer)
  if (props.node.healthy) {
    try {
      status.value = await nodeApi(props.node.id)<UpdateStatusDto>('/update')
    } catch {
      // Unreachable, or restarting into the new version: try again.
    }
  }
  timer = setTimeout(poll, updating.value || status.value?.phase === 'restarting' ? BUSY_POLL_MS : IDLE_POLL_MS)
}

async function update() {
  starting.value = true
  try {
    status.value = await nodeApi(props.node.id)<UpdateStatusDto>('/update', { method: 'POST' })
  } catch (e) {
    notifyError(`Couldn't update ${props.node.name}`, e, props.node.id)
  } finally {
    starting.value = false
    poll()
  }
}

onMounted(poll)
onBeforeUnmount(() => clearTimeout(timer))
// Back from a restart: look again at once.
watch(
  () => [props.node.healthy, props.node.version],
  () => poll(),
)
</script>

<template>
  <template v-if="shown && status">
    <span v-if="updating" class="flex items-center gap-1.5 text-xs text-muted-foreground">
      <LoaderCircle class="size-3.5 animate-spin" /> {{ progress }} <span class="num">{{ status.latest }}</span>
    </span>
    <Button
      v-else-if="status.available"
      variant="secondary"
      size="sm"
      class="h-7 gap-1.5 text-xs"
      :disabled="!!status.busy || !node.healthy"
      :title="status.busy ?? `Install ${status.latest}; the node restarts`"
      @click="update"
    >
      <Download class="size-3.5" /> Update to <span class="num">{{ status.latest }}</span>
    </Button>
    <span v-if="status.error && !updating" class="text-destructive" :title="status.error">
      <TriangleAlert class="size-3.5" />
    </span>
  </template>
</template>
