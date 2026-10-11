<script setup lang="ts">
import { computed } from 'vue'
import { Plus, X } from '@lucide/vue'
import type { ChannelConfig } from '@/types/generated/ChannelConfig'
import type { OutputConfig } from '@/types/generated/OutputConfig'
import type { DisplayDto } from '@/types/generated/DisplayDto'
import type { DeviceDto } from '@/types/generated/DeviceDto'
import type { DisplayIdle } from '@/types/generated/DisplayIdle'
import { Input } from '@/components/ui/input'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'
import CopyButton from '@/components/common/CopyButton.vue'
import { FRAMERATES, FRAMERATE_OPTIONS, RESOLUTIONS, RESOLUTION_OPTIONS } from '@/lib/videoPresets'
import { RTSP_PORT, hostPort, mountPath, srtListens } from '@/lib/streamUrl'

/** A playout channel: its fixed program format, and where it's sent. */
const props = defineProps<{
  /** The channel's name: what an NDI output is called unless renamed. */
  name: string
  /** Where receivers reach the node, for SRT listener and RTSP addresses. */
  host: string
  /** Output types the node can send (`ndi`, `srt`, `rtsp`, `display`). */
  outputTypes: string[]
  /** The node's screens, for display outputs. */
  displays: DisplayDto[]
  /** The node's devices; its audio outputs are offered for display outputs. */
  devices: DeviceDto[]
}>()

const cfg = defineModel<ChannelConfig>({ required: true })

type SrtOutput = Extract<OutputConfig, { type: 'srt' }>

const CHANNEL_OPTIONS = [
  { value: 1, label: 'Mono' },
  { value: 2, label: 'Stereo' },
  { value: 6, label: '5.1' },
  { value: 8, label: '7.1' },
]

const resolution = computed({
  get: () => `${cfg.value.format.width}x${cfg.value.format.height}`,
  set: (v: string) => {
    const r = RESOLUTIONS.find((r) => `${r.w}x${r.h}` === v)
    if (r) cfg.value = { ...cfg.value, format: { ...cfg.value.format, width: r.w, height: r.h } }
  },
})

const framerate = computed({
  get: () => `${cfg.value.format.fps_num}/${cfg.value.format.fps_den}`,
  set: (v: string) => {
    const r = FRAMERATES.find((r) => `${r.n}/${r.d}` === v)
    if (r) cfg.value = { ...cfg.value, format: { ...cfg.value.format, fps_num: r.n, fps_den: r.d } }
  },
})

function update(i: number, patch: Partial<OutputConfig>) {
  const outputs = cfg.value.outputs.map((o, j) => (j === i ? ({ ...o, ...patch } as OutputConfig) : o))
  cfg.value = { ...cfg.value, outputs }
}

function add(o: OutputConfig) {
  cfg.value = { ...cfg.value, outputs: [...cfg.value.outputs, o] }
}

/** A listener on the first port from 9000 this channel's outputs don't use. */
function addSrt() {
  const used = new Set(cfg.value.outputs.map((o) => (o.type === 'srt' ? hostPort(o.url).port : '')))
  let port = 9000
  while (used.has(String(port))) port++
  add({ type: 'srt', url: `srt://:${port}`, latency_ms: 200, bitrate_kbps: 8000, passphrase: null })
}

function remove(i: number) {
  cfg.value = { ...cfg.value, outputs: cfg.value.outputs.filter((_, j) => j !== i) }
}

// ── SRT ───────────────────────────────────────────────────────────────────────

function srtMode(o: SrtOutput) {
  return srtListens(o.url) ? 'listener' : 'caller'
}

function setSrtMode(i: number, o: SrtOutput, mode: unknown) {
  if (typeof mode !== 'string' || mode === srtMode(o)) return
  const { port } = hostPort(o.url)
  update(i, { url: mode === 'listener' ? `srt://:${port || 9000}` : `srt://10.0.0.20:${port || 9000}` })
}

// ── RTSP ──────────────────────────────────────────────────────────────────────

function rtspUrl(path: string | null) {
  return `rtsp://${props.host}:${RTSP_PORT}${mountPath(path, props.name)}`
}

/** Where a receiver connects, for a listener. */
function receiveAt(o: SrtOutput) {
  const { port } = hostPort(o.url)
  return srtListens(o.url) && port ? `srt://${props.host}:${port}` : null
}

// ── Display ───────────────────────────────────────────────────────────────────

/** Rates in mHz that show the channel without judder: its own, then double. */
const wantedRates = computed(() => {
  const { fps_num, fps_den } = cfg.value.format
  const mhz = Math.floor((fps_num * 1000) / Math.max(fps_den, 1))
  return [mhz, mhz * 2]
})

function hz(mhz: number) {
  return `${Number((mhz / 1000).toFixed(3))} Hz`
}

/** The mode a screen will be set to for this channel, or null if it has none. */
function matchingMode(d: DisplayDto) {
  const { width, height } = cfg.value.format
  for (const rate of wantedRates.value) {
    const m = d.modes.find((m) => m.width === width && m.height === height && Math.abs(m.refresh_mhz - rate) <= 10)
    if (m) return m
  }
  return null
}

const displayOptions = computed(() =>
  props.displays.map((d) => ({ value: d.id, label: `${d.name}${d.primary ? ' (main screen)' : ''}` })),
)

function displayFor(id: string) {
  return props.displays.find((d) => d.id === id)
}

/** What the screen will do with this channel's format. */
function displayHint(id: string) {
  const d = displayFor(id)
  if (!d) return id ? `${id} isn't connected now; the output starts when it is.` : 'Pick a screen.'
  const m = matchingMode(d)
  const { width, height } = cfg.value.format
  if (m) return `The screen is set to ${m.width}×${m.height} at ${hz(m.refresh_mhz)} while this plays.`
  return `This screen has no ${width}×${height} mode at the channel's rate: it stays at ${d.width}×${d.height} at ${hz(d.refresh_mhz)} and the picture is scaled.`
}

const IDLE_OPTIONS: { value: DisplayIdle; label: string }[] = [
  { value: 'black', label: 'Black' },
  { value: 'desktop', label: 'The desktop (output hides)' },
]

const audioOutputs = computed(() => props.devices.filter((d) => d.kind === 'output'))
/** "No audio": select items can't have an empty value. */
const NO_AUDIO = 'none'
const audioOptions = computed(() => [
  { value: NO_AUDIO, label: 'No audio' },
  ...audioOutputs.value.map((d) => ({ value: d.key, label: d.name })),
])

/** An external screen first (the main one is usually the operator's), with its own audio when the names match. */
function addDisplay() {
  const used = new Set(cfg.value.outputs.map((o) => (o.type === 'display' ? o.display : '')))
  const free = props.displays.filter((d) => !used.has(d.id))
  const screen = free.find((d) => !d.primary) ?? free[0]
  const audio = screen && audioOutputs.value.find((a) => a.name.toLowerCase().includes(screen.name.toLowerCase()))
  add({ type: 'display', display: screen?.id ?? '', audio_device: audio?.key ?? null, idle: 'black' })
}

const TYPE_LABELS: Record<string, string> = { ndi: 'NDI', srt: 'SRT', rtsp: 'RTSP', display: 'Screen' }
</script>

<template>
  <div class="col-span-2 grid grid-cols-2 gap-3">
    <FormField label="Resolution">
      <OptionSelect v-model="resolution" :options="RESOLUTION_OPTIONS" />
    </FormField>
    <FormField label="Frame rate">
      <OptionSelect v-model="framerate" :options="FRAMERATE_OPTIONS" />
    </FormField>
    <FormField label="Audio">
      <OptionSelect v-model="cfg.audio_channels" :options="CHANNEL_OPTIONS" />
    </FormField>
    <p class="col-span-2 -mt-1 text-xs text-muted-foreground">
      Every clip is scaled and converted to this format, so outputs never change format between clips.
    </p>

    <div class="col-span-2 flex flex-col gap-1.5">
      <span class="text-xs text-muted-foreground">Outputs</span>
      <div v-for="(o, i) in cfg.outputs" :key="i" class="flex flex-col gap-2 rounded-md border border-border px-2.5 py-1.5">
        <div class="flex items-center gap-2">
          <span class="text-[10px] font-semibold uppercase tracking-wide text-muted-foreground w-12 shrink-0">{{ TYPE_LABELS[o.type] }}</span>
          <Input
            v-if="o.type === 'ndi'"
            :model-value="o.ndi_name ?? ''"
            class="h-7 flex-1"
            :placeholder="props.name.trim() || 'Channel name'"
            :aria-label="`NDI name for output ${i + 1}`"
            @update:model-value="(v) => update(i, { ndi_name: String(v) || null })"
          />
          <OptionSelect
            v-else-if="o.type === 'display'"
            class="flex-1 min-w-0"
            :model-value="o.display"
            :options="displayFor(o.display) || !o.display ? displayOptions : [{ value: o.display, label: o.display }, ...displayOptions]"
            :aria-label="`Screen for output ${i + 1}`"
            @update:model-value="(v) => update(i, { display: String(v) })"
          />
          <Input
            v-else-if="o.type === 'rtsp'"
            :model-value="o.path ?? ''"
            class="h-7 flex-1 font-mono text-[0.6875rem]"
            :placeholder="mountPath(null, props.name)"
            :aria-label="`RTSP path for output ${i + 1}`"
            @update:model-value="(v) => update(i, { path: String(v) || null })"
          />
          <template v-else>
            <ToggleGroup
              :model-value="srtMode(o)"
              type="single"
              variant="segmented"
              class="flex-1 min-w-0"
              @update:model-value="(m) => setSrtMode(i, o, m)"
            >
              <ToggleGroupItem value="listener" class="flex-1 h-7 text-xs">Listener</ToggleGroupItem>
              <ToggleGroupItem value="caller" class="flex-1 h-7 text-xs">Caller</ToggleGroupItem>
            </ToggleGroup>
          </template>
          <button
            type="button"
            class="size-7 grid place-items-center rounded text-muted-foreground hover:text-foreground hover:bg-accent"
            :aria-label="`Remove output ${i + 1}`"
            @click="remove(i)"
          >
            <X class="size-3.5" />
          </button>
        </div>

        <template v-if="o.type === 'display'">
          <p class="text-xs text-muted-foreground">{{ displayHint(o.display) }}</p>
          <p v-if="displayFor(o.display)?.primary" class="text-xs text-warning">
            This is the node's main screen. The output covers all of it, menu bar and Dock included, until the channel
            stops or the output is removed — from another device's browser if this is the only screen.
          </p>
          <FormField label="Audio">
            <OptionSelect
              :model-value="o.audio_device ?? NO_AUDIO"
              :options="audioOptions"
              @update:model-value="(v) => update(i, { audio_device: v === NO_AUDIO ? null : String(v) })"
            />
          </FormField>
          <p class="text-xs text-muted-foreground">
            For an HDMI→SDI converter, pick the screen's own HDMI audio so it's embedded.
          </p>
          <FormField label="When nothing is loaded, show">
            <OptionSelect
              :model-value="o.idle"
              :options="IDLE_OPTIONS"
              @update:model-value="(v) => update(i, { idle: v as DisplayIdle })"
            />
          </FormField>
          <p class="text-xs text-muted-foreground">
            <template v-if="o.idle === 'desktop'">
              The output appears when a clip is cued and hides when the channel stops. Not for converters: the signal
              drops while it's hidden.
            </template>
            <template v-else>Keeps the signal steady, which converters and switchers need.</template>
          </p>
        </template>

        <template v-if="o.type === 'rtsp'">
          <div class="grid grid-cols-3 gap-2">
            <FormField label="Bitrate (kbps)">
              <Input
                :model-value="o.bitrate_kbps"
                type="number"
                min="100"
                max="100000"
                class="h-7"
                @update:model-value="(v) => update(i, { bitrate_kbps: Number(v) })"
              />
            </FormField>
          </div>
          <div class="flex flex-wrap items-center gap-1 text-xs">
            <span class="text-muted-foreground">Viewers open</span>
            <span class="font-mono">{{ rtspUrl(o.path) }}</span>
            <CopyButton :value="rtspUrl(o.path)" />
          </div>
          <p class="text-xs text-muted-foreground">Encoded only while someone is watching; every viewer shares one encode.</p>
        </template>

        <template v-if="o.type === 'srt'">
          <Input
            :model-value="o.url"
            class="h-7 font-mono text-[0.6875rem]"
            placeholder="srt://:9000"
            :aria-label="`SRT URL for output ${i + 1}`"
            @update:model-value="(v) => update(i, { url: String(v) })"
          />
          <div class="grid grid-cols-3 gap-2">
            <FormField label="Bitrate (kbps)">
              <Input
                :model-value="o.bitrate_kbps"
                type="number"
                min="100"
                max="100000"
                class="h-7"
                @update:model-value="(v) => update(i, { bitrate_kbps: Number(v) })"
              />
            </FormField>
            <FormField label="Latency (ms)">
              <Input
                :model-value="o.latency_ms"
                type="number"
                min="20"
                max="8000"
                class="h-7"
                @update:model-value="(v) => update(i, { latency_ms: Number(v) })"
              />
            </FormField>
            <FormField label="Passphrase">
              <Input
                :model-value="o.passphrase ?? ''"
                type="password"
                autocomplete="off"
                placeholder="Optional"
                class="h-7"
                @update:model-value="(v) => update(i, { passphrase: String(v) || null })"
              />
            </FormField>
          </div>
          <div v-if="receiveAt(o)" class="flex flex-wrap items-center gap-1 text-xs">
            <span class="text-muted-foreground">Receivers connect to</span>
            <span class="font-mono">{{ receiveAt(o) }}<template v-if="o.passphrase">?passphrase=…</template></span>
            <CopyButton :value="receiveAt(o)!" />
          </div>
          <p v-else class="text-xs text-muted-foreground">Sends to the receiver at this address, retrying until it answers.</p>
        </template>
      </div>

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
            :disabled="!props.outputTypes.includes('display') || !props.displays.length"
            class="flex-col items-start gap-0"
            @select="addDisplay"
          >
            <span>Screen</span>
            <span class="text-[11px] text-muted-foreground">Fullscreen on a screen attached to the node, e.g. into an HDMI→SDI converter</span>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <p class="text-xs text-muted-foreground">
        NDI receivers see it as <span class="font-mono">NODE (name)</span>. The program can also be recorded from Record like any source.
      </p>
    </div>
  </div>
</template>
