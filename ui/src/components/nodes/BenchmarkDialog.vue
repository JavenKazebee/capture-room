<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { toast } from 'vue-sonner'
import { Film, FolderOpen, Loader2 } from '@lucide/vue'
import { errorMessage } from '@/composables/useApi'
import { useCapacityStore } from '@/stores/capacity'
import { useNodesStore } from '@/stores/nodes'
import { blankLeg, DEFAULT_PRESET, usePresetsStore } from '@/stores/presets'
import { REPLAY_CODECS } from '@/lib/codecs'
import { useReplayStore } from '@/stores/replay'
import { useSourcesStore } from '@/stores/sources'
import type { ConfiguredSourceDto } from '@/types/generated/ConfiguredSourceDto'
import type { OutputConfig } from '@/types/generated/OutputConfig'
import type { ReplayCodec } from '@/types/generated/ReplayCodec'
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
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'
import FileBrowseDialog from '@/components/sources/FileBrowseDialog.vue'

/** Start a benchmark on one node. Mount it with `v-if`. */
const props = defineProps<{ nodeId: string }>()
const emit = defineEmits<{ close: [] }>()

const capacity = useCapacityStore()
const nodes = useNodesStore()
const presets = usePresetsStore()
const sources = useSourcesStore()
const replay = useReplayStore()

/** What each feed does: record with a preset, keep a replay buffer, or run as a playout channel. */
const measure = ref<'record' | 'replay' | 'channel'>('record')
const measureOptions = [
  { value: 'record' as const, label: 'Recording' },
  { value: 'replay' as const, label: 'Replay buffers' },
  { value: 'channel' as const, label: 'Playout channels' },
]

/** Outputs that always encode; RTSP/WHEP encode only for viewers and a display only draws, so they're left out. */
const channelOutputs = ref<'none' | 'ndi' | 'srt' | 'ndi_srt'>('ndi')
const channelOutputOptions = [
  { value: 'ndi' as const, label: 'NDI' },
  { value: 'srt' as const, label: 'SRT' },
  { value: 'ndi_srt' as const, label: 'NDI + SRT' },
  { value: 'none' as const, label: 'None (program only)' },
]
const srtBitrate = ref(8000)

function channelConfigs(): OutputConfig[] {
  const ndi: OutputConfig = { type: 'ndi', ndi_name: null }
  const srt: OutputConfig = { type: 'srt', url: '', latency_ms: 120, bitrate_kbps: srtBitrate.value, passphrase: null }
  return { none: [], ndi: [ndi], srt: [srt], ndi_srt: [ndi, srt] }[channelOutputs.value]
}

/** Measure like the node's first channel, if it has one. */
function likeChannel(outputs: OutputConfig[]) {
  const ndi = outputs.some((o) => o.type === 'ndi')
  const srt = outputs.find((o) => o.type === 'srt')
  if (srt?.type === 'srt') srtBitrate.value = srt.bitrate_kbps
  channelOutputs.value = ndi && srt ? 'ndi_srt' : ndi ? 'ndi' : srt ? 'srt' : 'none'
}

/** The node's default codec. */
const codec = ref<ReplayCodec>(replay.codecsOf(props.nodeId)[0] ?? 'prores_proxy')
const codecOptions = computed(() =>
  replay.codecsOf(props.nodeId).map((c) => ({ value: c, label: REPLAY_CODECS[c].label })),
)

const presetId = ref<string>(DEFAULT_PRESET.id)

const mediaPath = ref('')
const browsing = ref(false)
/** The node's file sources, offered as footage. */
const fileSources = ref<(ConfiguredSourceDto & { path: string })[]>([])

const maxFeeds = ref(64)
const stepSecs = ref(20)
const threshold = ref(0.5)

const saving = ref(false)
const error = ref<string | null>(null)

onMounted(async () => {
  const configs = await sources.configs(props.nodeId).catch(() => [])
  const channel = configs.find((c) => c.config.type === 'channel')
  if (channel?.config.type === 'channel') likeChannel(channel.config.outputs)
  fileSources.value = configs.flatMap((c) => (c.config.type === 'file' ? [{ ...c, path: c.config.path }] : []))
  if (!mediaPath.value && fileSources.value[0]) mediaPath.value = fileSources.value[0].path
})

/** Rough run length: a quick check per doubling, about as many full steps to narrow down, plus warm-ups. */
const minutes = computed(() => {
  const doublings = Math.ceil(Math.log2(Math.max(2, maxFeeds.value))) + 1
  const secs = doublings * (5 + 8) + doublings * (stepSecs.value + 8)
  return Math.max(1, Math.round(secs / 60))
})

async function start() {
  error.value = null
  if (!mediaPath.value.trim()) {
    error.value = 'Pick the footage to play.'
    return
  }
  const forReplay = measure.value === 'replay'
  const forChannel = measure.value === 'channel'
  if (forReplay && !codecOptions.value.length) {
    error.value = "This node can't keep replay buffers: it has none of the codecs installed."
    return
  }
  const preset = forReplay || forChannel ? null : (presets.presets.find((p) => p.id === presetId.value) ?? null)
  saving.value = true
  try {
    await capacity.start(props.nodeId, {
      preset_id: preset?.id ?? null,
      preset_name: preset?.name ?? null,
      outputs: forReplay || forChannel ? [] : preset ? preset.outputs : [blankLeg()],
      replay: forReplay ? codec.value : null,
      channel: forChannel ? channelConfigs() : null,
      media_path: mediaPath.value.trim(),
      max_feeds: maxFeeds.value,
      step_secs: stepSecs.value,
      drop_threshold_pct: threshold.value,
    })
    toast.success(`Benchmarking ${nodes.nameOf(props.nodeId)}`)
    emit('close')
  } catch (e) {
    error.value = errorMessage(e, "Couldn't start the benchmark")
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <Dialog :open="true" @update:open="(v) => !v && emit('close')">
    <DialogContent class="sm:max-w-xl gap-4 max-h-[90vh] overflow-y-auto">
      <DialogHeader>
        <DialogTitle>Benchmark {{ nodes.nameOf(nodeId) }}</DialogTitle>
        <DialogDescription>
          Records more and more copies of the footage until frames drop, to find how many feeds this node can record
          with a preset, keep replay buffers of, or play out on channels.
        </DialogDescription>
      </DialogHeader>

      <form class="grid grid-cols-3 gap-3" @submit.prevent="start">
        <FormField label="Measure">
          <OptionSelect v-model="measure" :options="measureOptions" />
        </FormField>
        <FormField v-if="measure === 'record'" label="Preset" class="col-span-2">
          <OptionSelect v-model="presetId" :options="presets.options" />
        </FormField>
        <template v-else-if="measure === 'channel'">
          <FormField label="Outputs">
            <OptionSelect v-model="channelOutputs" :options="channelOutputOptions" />
          </FormField>
          <FormField v-if="channelOutputs === 'srt' || channelOutputs === 'ndi_srt'" label="SRT bitrate (kb/s)">
            <Input v-model.number="srtBitrate" type="number" min="500" max="100000" step="500" class="num" />
          </FormField>
        </template>
        <FormField v-else label="Codec" class="col-span-2">
          <OptionSelect v-if="codecOptions.length" v-model="codec" :options="codecOptions" />
          <p v-else class="text-xs text-muted-foreground h-8 flex items-center">This node has no replay codecs.</p>
        </FormField>

        <FormField label="Footage" class="col-span-3">
          <div class="flex gap-1.5">
            <Input v-model="mediaPath" class="font-mono text-[0.6875rem]" placeholder="/path/on/the/node/clip.mov" />
            <Button type="button" variant="outline" class="gap-1.5" @click="browsing = true">
              <FolderOpen class="size-3.5" /> Browse
            </Button>
          </div>
        </FormField>
        <div v-if="fileSources.length" class="col-span-3 -mt-1 flex flex-wrap gap-1">
          <Button
            v-for="f in fileSources"
            :key="f.id"
            type="button"
            variant="outline"
            size="sm"
            class="h-6 gap-1 px-2 text-xs"
            :class="mediaPath === f.path && 'border-primary text-primary'"
            :title="f.path"
            @click="mediaPath = f.path"
          >
            <Film class="size-3" /> {{ f.name }}
          </Button>
        </div>
        <p class="col-span-3 text-xs text-muted-foreground">
          <template v-if="measure === 'channel'">
            Use real footage at your channels' format: each channel runs at the footage's size and rate, and results
            only count exactly for channels of that format.
          </template>
          <template v-else>
            Use real footage at the format your sources send: test patterns compress unrealistically, and results only
            count for recordings in the same format.
          </template>
        </p>

        <FormField label="Feed limit">
          <Input v-model.number="maxFeeds" type="number" min="1" max="128" class="num" />
        </FormField>
        <FormField label="Seconds per step">
          <Input v-model.number="stepSecs" type="number" min="5" max="300" class="num" />
        </FormField>
        <FormField label="Drop limit (%)">
          <Input v-model.number="threshold" type="number" min="0.01" max="10" step="0.1" class="num" />
        </FormField>

        <div class="col-span-3 space-y-1.5 rounded-md border border-border bg-muted/40 p-3 text-xs text-muted-foreground">
          <p>
            Doubles the feeds with quick checks until frames drop, then narrows down with
            <span class="num">{{ stepSecs }}</span> s steps to the most feeds that keep within
            <span class="num">{{ threshold }}%</span> dropped frames, up to <span class="num">{{ maxFeeds }}</span>.
            Usually about <span class="num">{{ minutes }}</span> min.
          </p>
          <p v-if="measure === 'record'">
            Files are written beside where the preset records, so the same disks are measured, and deleted afterwards.
            Starting a recording or a replay buffer on this node cancels the benchmark.
          </p>
          <p v-else-if="measure === 'channel'">
            Each feed is a playout channel playing the footage on loop, with its own outputs: NDI as “Benchmark 1”, “Benchmark
            2”… (seen on the network while it runs), SRT listening on a free local port. RTSP, WHEP and display outputs
            are left out: they encode only for viewers, or need a screen. Channels already running stay on, and take
            some of the node.
          </p>
          <p v-else>
            Each feed keeps a replay buffer beside the node's real ones, so the same disk is measured; old segments are
            deleted as they would be, and everything afterwards. Starting a recording or a replay buffer on this node
            cancels the benchmark.
          </p>
        </div>
        <button type="submit" hidden />
      </form>

      <DialogFooter class="items-center">
        <p v-if="error" class="mr-auto text-xs text-destructive">{{ error }}</p>
        <Button variant="outline" :disabled="saving" @click="emit('close')">Cancel</Button>
        <Button :disabled="saving" class="gap-1.5" @click="start">
          <Loader2 v-if="saving" class="size-3.5 animate-spin" />
          Start
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>

  <FileBrowseDialog
    v-if="browsing"
    :node-id="nodeId"
    :start="mediaPath || undefined"
    @close="browsing = false"
    @select="(p) => (mediaPath = p)"
  />
</template>
