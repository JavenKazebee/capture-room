<script setup lang="ts">
import { computed } from 'vue'
import { Plus } from '@lucide/vue'
import type { ChannelConfig } from '@/types/generated/ChannelConfig'
import type { OutputConfig } from '@/types/generated/OutputConfig'
import type { DisplayDto } from '@/types/generated/DisplayDto'
import type { DeviceDto } from '@/types/generated/DeviceDto'
import type { DecklinkStatusDto } from '@/types/generated/DecklinkStatusDto'
import { subDeviceOptions } from '@/lib/decklink'
import { hostPort } from '@/lib/streamUrl'
import { Input } from '@/components/ui/input'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import ChannelFormatFields from './channel/ChannelFormatFields.vue'
import DisplayOutputFields from './channel/DisplayOutputFields.vue'
import OutputRow from './channel/OutputRow.vue'
import RtspWhepOutputFields from './channel/RtspWhepOutputFields.vue'
import SdiOutputFields from './channel/SdiOutputFields.vue'
import SrtOutputFields from './channel/SrtOutputFields.vue'

/** A playout channel: its fixed program format, and where it's sent. */
const props = defineProps<{
  /** The channel's name: what an NDI output is called unless renamed. */
  name: string
  /** Where receivers reach the node, for SRT listener and RTSP addresses. */
  host: string
  /** The node's API origin as receivers reach it, for WHEP addresses. */
  origin: string
  /** Output types the node can send (`ndi`, `srt`, `rtsp`, `whep`, `display`). */
  outputTypes: string[]
  /** The node's screens, for display outputs. */
  displays: DisplayDto[]
  /** The node's devices; its audio outputs are offered for display outputs. */
  devices: DeviceDto[]
  /** The node's Decklink cards, for SDI outputs. */
  decklink: DecklinkStatusDto
  nodeId: string
  /** The channel being edited, so its own SDI outputs aren't flagged as taken. */
  channelId: string | null
}>()
const emit = defineEmits<{ refreshDecklink: [] }>()

const cfg = defineModel<ChannelConfig>({ required: true })

function update(i: number, patch: Partial<OutputConfig>) {
  const outputs = cfg.value.outputs.map((o, j) => (j === i ? ({ ...o, ...patch } as OutputConfig) : o))
  cfg.value = { ...cfg.value, outputs }
}

function add(o: OutputConfig) {
  cfg.value = { ...cfg.value, outputs: [...cfg.value.outputs, o] }
}

function remove(i: number) {
  cfg.value = { ...cfg.value, outputs: cfg.value.outputs.filter((_, j) => j !== i) }
}

/** A listener on the first port from 9000 this channel's outputs don't use. */
function addSrt() {
  const used = new Set(cfg.value.outputs.map((o) => (o.type === 'srt' ? hostPort(o.url).port : '')))
  let port = 9000
  while (used.has(String(port))) port++
  add({ type: 'srt', url: `srt://:${port}`, latency_ms: 200, bitrate_kbps: 8000, passphrase: null })
}

/** An external screen first (the main one is usually the operator's), with its own audio when the names match. */
function addDisplay() {
  const used = new Set(cfg.value.outputs.map((o) => (o.type === 'display' ? o.display : '')))
  const free = props.displays.filter((d) => !used.has(d.id))
  const screen = free.find((d) => !d.primary) ?? free[0]
  const audio =
    screen && props.devices.find((a) => a.kind === 'output' && a.name.toLowerCase().includes(screen.name.toLowerCase()))
  add({ type: 'display', display: screen?.id ?? '', audio_device: audio?.key ?? null, idle: 'black' })
}

const sdiOptions = computed(() => subDeviceOptions(props.decklink, true, props.channelId))

/** A free playout sub-device first. */
function addSdi() {
  const used = new Set(cfg.value.outputs.map((o) => (o.type === 'decklink' ? o.device : '')))
  const free = sdiOptions.value.filter((o) => !used.has(o.value))
  const pick = free.find((o) => !o.label.includes(' — used by ')) ?? free[0]
  add({ type: 'decklink', device: pick?.value ?? '', mode: null })
}
</script>

<template>
  <div class="col-span-2 grid grid-cols-2 gap-3">
    <ChannelFormatFields v-model:format="cfg.format" v-model:audio-channels="cfg.audio_channels" />

    <div class="col-span-2 flex flex-col gap-1.5">
      <span class="text-xs text-muted-foreground">Outputs</span>
      <template v-for="(o, i) in cfg.outputs" :key="i">
        <OutputRow v-if="o.type === 'ndi'" label="NDI" :index="i" @remove="remove(i)">
          <template #control>
            <Input
              :model-value="o.ndi_name ?? ''"
              class="h-7 flex-1"
              :placeholder="props.name.trim() || 'Channel name'"
              :aria-label="`NDI name for output ${i + 1}`"
              @update:model-value="(v) => update(i, { ndi_name: String(v) || null })"
            />
          </template>
        </OutputRow>
        <DisplayOutputFields
          v-else-if="o.type === 'display'"
          :output="o"
          :index="i"
          :format="cfg.format"
          :displays="displays"
          :devices="devices"
          @update="(p) => update(i, p)"
          @remove="remove(i)"
        />
        <SdiOutputFields
          v-else-if="o.type === 'decklink'"
          :output="o"
          :index="i"
          :format="cfg.format"
          :audio-channels="cfg.audio_channels"
          :decklink="decklink"
          :node-id="nodeId"
          :channel-id="channelId"
          @update="(p) => update(i, p)"
          @remove="remove(i)"
          @refresh-decklink="emit('refreshDecklink')"
        />
        <RtspWhepOutputFields
          v-else-if="o.type === 'rtsp' || o.type === 'whep'"
          :output="o"
          :index="i"
          :name="name"
          :host="host"
          :origin="origin"
          @update="(p) => update(i, p)"
          @remove="remove(i)"
        />
        <SrtOutputFields
          v-else
          :output="o"
          :index="i"
          :host="host"
          @update="(p) => update(i, p)"
          @remove="remove(i)"
        />
      </template>

      <DropdownMenu>
        <DropdownMenuTrigger class="flex items-center gap-1 text-xs text-primary hover:underline w-fit">
          <Plus class="size-3.5" /> Add output
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" class="w-64">
          <DropdownMenuItem
            :disabled="!props.outputTypes.includes('ndi')"
            class="flex-col items-start gap-0"
            @select="add({ type: 'ndi', ndi_name: null })"
          >
            <span>NDI</span>
            <span class="text-[11px] text-muted-foreground">Uncompressed, on the local network</span>
          </DropdownMenuItem>
          <DropdownMenuItem :disabled="!props.outputTypes.includes('srt')" class="flex-col items-start gap-0" @select="addSrt">
            <span>SRT</span>
            <span class="text-[11px] text-muted-foreground">H.264 and AAC, to vMix, OBS, a decoder or across the internet</span>
          </DropdownMenuItem>
          <DropdownMenuItem
            :disabled="!props.outputTypes.includes('rtsp')"
            class="flex-col items-start gap-0"
            @select="add({ type: 'rtsp', path: null, bitrate_kbps: 8000 })"
          >
            <span>RTSP</span>
            <span class="text-[11px] text-muted-foreground">Served by the node, for VLC, decoders and NVRs to pull</span>
          </DropdownMenuItem>
          <DropdownMenuItem
            :disabled="!props.outputTypes.includes('whep')"
            class="flex-col items-start gap-0"
            @select="add({ type: 'whep', path: null, bitrate_kbps: 8000 })"
          >
            <span>WHEP</span>
            <span class="text-[11px] text-muted-foreground">WebRTC, low latency, to browsers and WHEP players</span>
          </DropdownMenuItem>
          <DropdownMenuItem
            :disabled="!props.outputTypes.includes('display') || !props.displays.length"
            class="flex-col items-start gap-0"
            @select="addDisplay"
          >
            <span>Screen</span>
            <span class="text-[11px] text-muted-foreground">Fullscreen on a screen attached to the node, e.g. into an HDMI→SDI converter</span>
          </DropdownMenuItem>
          <DropdownMenuItem
            :disabled="!props.outputTypes.includes('decklink') || !sdiOptions.length"
            class="flex-col items-start gap-0"
            @select="addSdi"
          >
            <span>SDI (Decklink)</span>
            <span class="text-[11px] text-muted-foreground">
              {{ decklink.unavailable ?? 'Out of a Blackmagic Decklink card, with embedded audio' }}
            </span>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <p class="text-xs text-muted-foreground">
        NDI receivers see it as <span class="font-mono">NODE (name)</span>. The program can also be recorded from Record like any source.
      </p>
    </div>
  </div>
</template>
