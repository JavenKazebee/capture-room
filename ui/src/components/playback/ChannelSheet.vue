<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { toast } from 'vue-sonner'
import { Trash2 } from '@lucide/vue'
import { useSourcesStore } from '@/stores/sources'
import { nodeAddress, useNodesStore } from '@/stores/nodes'
import { usePlayoutStore } from '@/stores/playout'
import { capacityWarningText, useCapacityStore } from '@/stores/capacity'
import { notifyWarn } from '@/lib/notify'
import { errorMessage, sourceKey } from '@/composables/useApi'
import { useNodeDevices } from '@/composables/useNodeDevices'
import type { ChannelConfig } from '@/types/generated/ChannelConfig'
import type { DisplayDto } from '@/types/generated/DisplayDto'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'
import EditSheet from '@/components/common/EditSheet.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import ChannelFields from '@/components/sources/ChannelFields.vue'

/**
 * Create a playout channel, or edit one: its name, format and outputs. A
 * channel counts against its node's benchmarked capacity, so saving one warns
 * (never blocks) when that runs short.
 */
const props = defineProps<{
  /** The channel to edit; a new one when unset. */
  editing?: { nodeId: string; id: string } | null
}>()
const emit = defineEmits<{
  close: []
  /** The channel's source key, after a save. */
  saved: [key: string]
}>()

const sources = useSourcesStore()
const nodes = useNodesStore()
const playout = usePlayoutStore()
const capacity = useCapacityStore()

function blankChannel(): ChannelConfig {
  return {
    format: { width: 1920, height: 1080, fps_num: 30, fps_den: 1 },
    audio_channels: 2,
    outputs: [{ type: 'ndi', ndi_name: null }],
  }
}

const nodeId = ref(props.editing?.nodeId ?? nodes.self?.node_id ?? nodes.nodes[0]?.id ?? '')
const name = ref('')
const channel = ref<ChannelConfig>(blankChannel())
const loading = ref(!!props.editing)
const saving = ref(false)
const error = ref<string | null>(null)
const confirmDelete = ref(false)
/** Playing or paused on a clip: deleting takes it off air. */
const onAir = computed(() => {
  if (!props.editing) return false
  const state = playout.status.get(sourceKey(props.editing.nodeId, props.editing.id))?.state
  return state === 'playing' || state === 'paused'
})
const deleteDescription = computed(
  () =>
    `It stops${onAir.value ? ", taking what it's playing off air," : ''} and is removed from its node with its playlist and outputs. Media and recordings are kept.`,
)

const node = computed(() => nodes.nodes.find((n) => n.id === nodeId.value))
const nodeOptions = computed(() => nodes.nodes.map((n) => ({ value: n.id, label: nodes.labelOf(n.id) })))
/** The node's address as receivers on the network see it. */
const host = computed(() => nodeAddress(node.value?.url).hostname)
/** The node's API as receivers on the network reach it, for WHEP outputs. */
const origin = computed(() => nodeAddress(node.value?.url).origin)
/** Output types the node's channels can send. */
const outputTypes = computed(
  () => node.value?.source_types.find((t) => t.source_type === 'channel')?.protocols ?? ['ndi', 'srt', 'rtsp'],
)
const unavailable = computed(() => {
  const t = node.value?.source_types.find((t) => t.source_type === 'channel')
  return t?.missing.length ? `This node can't run channels: it lacks ${t.missing.join(', ')}.` : null
})

const devices = useNodeDevices(nodeId)
const displays = ref<DisplayDto[]>([])

/** The node's screens and audio outputs, for display outputs. */
function refreshScreens() {
  devices.refresh()
  const id = nodeId.value
  if (!id) return
  sources
    .listDisplays(id)
    .then((d) => {
      if (nodeId.value === id) displays.value = d
    })
    .catch(() => (displays.value = []))
}
watch(nodeId, refreshScreens)

onMounted(async () => {
  refreshScreens()
  if (!props.editing) return
  try {
    const cfg = await sources.getConfig(props.editing.nodeId, props.editing.id)
    if (cfg?.config.type === 'channel') {
      const { type: _type, ...rest } = cfg.config
      name.value = cfg.name
      channel.value = rest
    } else {
      error.value = "This channel's settings couldn't be found on its node."
    }
  } catch (e) {
    error.value = errorMessage(e, "Couldn't load the channel's settings from its node.")
  } finally {
    loading.value = false
  }
})

async function save() {
  if (saving.value || loading.value) return
  if (!name.value.trim()) {
    error.value = 'Name is required.'
    return
  }
  const req = { name: name.value, config: { type: 'channel' as const, ...channel.value } }
  saving.value = true
  error.value = null
  try {
    let id: string
    if (props.editing) {
      id = props.editing.id
      await sources.updateSource(nodeId.value, id, req)
      toast.success(`Saved ${req.name}`)
    } else {
      id = (await sources.createSource(nodeId.value, req)).id
      toast.success(`Added ${req.name}`)
    }
    emit('saved', sourceKey(nodeId.value, id))
    warnLoad(nodeId.value, id, channel.value)
    emit('close')
  } catch (e) {
    error.value = errorMessage(e, 'Save failed.')
  } finally {
    saving.value = false
  }
}

/** Warn if the node is near or past its benchmarked capacity with channel `id` running as `config`. */
async function warnLoad(node: string, id: string, config: ChannelConfig) {
  const check = await capacity.checkChannel(node, config, id).catch(() => null)
  if (!check) return
  const text = capacityWarningText(nodes.nameOf(node), check.load_after)
  if (text) notifyWarn(text, node)
  if (capacity.capacity.has(node)) capacity.load(node)
}

async function destroy() {
  if (!props.editing) return
  try {
    await sources.deleteSource(props.editing.nodeId, props.editing.id)
    toast.success(`Deleted ${name.value}`)
    emit('close')
  } catch (e) {
    error.value = errorMessage(e, 'Delete failed.')
  }
}
</script>

<template>
  <EditSheet
    :title="editing ? `Edit ${name || 'channel'}` : 'New playout channel'"
    description="Plays clips from the node's media library out at a fixed format. Black and silence when nothing plays. It's also a source you can record."
    :error="error ?? unavailable"
    :saving="saving"
    :no-save="loading"
    @close="emit('close')"
    @save="save"
  >
    <div v-if="loading" class="text-sm text-muted-foreground">Loading…</div>
    <div v-else class="grid grid-cols-2 gap-3">
      <FormField v-if="!editing && nodes.nodes.length > 1" label="Node" class="col-span-2">
        <OptionSelect v-model="nodeId" :options="nodeOptions" />
      </FormField>

      <FormField label="Name" class="col-span-2">
        <Input v-model="name" placeholder="Playout A" />
      </FormField>

      <ChannelFields
        v-model="channel"
        :name="name"
        :host="host"
        :origin="origin"
        :output-types="outputTypes"
        :displays="displays"
        :devices="devices.devices.value"
      />
    </div>
    <template v-if="editing && !loading" #footer-start>
      <Button
        type="button"
        variant="ghost"
        class="gap-1.5 text-destructive hover:text-destructive hover:bg-destructive/10"
        :disabled="saving"
        @click="confirmDelete = true"
      >
        <Trash2 class="size-3.5" /> Delete channel
      </Button>
    </template>
  </EditSheet>

  <ConfirmDialog
    :open="confirmDelete"
    :title="`Delete playout channel ${name}?`"
    :description="deleteDescription"
    @update:open="(v) => !v && (confirmDelete = false)"
    @confirm="destroy"
  />
</template>
