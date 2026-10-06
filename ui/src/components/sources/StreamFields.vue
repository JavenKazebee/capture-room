<script setup lang="ts">
import { computed, ref } from 'vue'
import { ChevronRight } from '@lucide/vue'
import type { StreamSourceConfig } from '@/types/generated/StreamSourceConfig'
import type { DeviceDto } from '@/types/generated/DeviceDto'
import type { RtspTransport } from '@/types/generated/RtspTransport'
import { Input } from '@/components/ui/input'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'
import CopyButton from '@/components/common/CopyButton.vue'
import AudioPlanField from './AudioPlanField.vue'
import LiveFormatField from './LiveFormatField.vue'

/**
 * A network stream, URL first: the fields that matter for its protocol
 * appear under it. Mirrors the node's URL rules (`sources/stream.rs`): SRT
 * listens when the URL has no host or says `mode=listener`; UDP always
 * listens.
 */
const props = defineProps<{
  /** URL schemes the node can open. */
  protocols: string[]
  /** Where senders reach the node, for listener addresses. */
  host: string
  devices: DeviceDto[]
}>()

const cfg = defineModel<StreamSourceConfig>({ required: true })

const EXAMPLES = [
  { scheme: 'rtsp', url: 'rtsp://camera.local:554/stream1', label: 'RTSP camera' },
  { scheme: 'srt', url: 'srt://:9000', label: 'SRT listener' },
  { scheme: 'srt', url: 'srt://10.0.0.20:9000', label: 'SRT caller' },
  { scheme: 'rtmp', url: 'rtmp://server/live/key', label: 'RTMP pull' },
  { scheme: 'https', url: 'https://example.com/live.m3u8', label: 'HLS' },
  { scheme: 'udp', url: 'udp://@:5000', label: 'UDP MPEG-TS' },
]
const examples = computed(() => EXAMPLES.filter((e) => props.protocols.includes(e.scheme)))

const url = computed(() => cfg.value.url.trim())
const scheme = computed(() => url.value.match(/^([a-z][a-z0-9+.-]*):\/\//i)?.[1]?.toLowerCase() ?? '')

function hostPort(u: string) {
  const rest = u.split('://')[1] ?? ''
  const auth = rest.split(/[/?]/)[0].split('@').pop()!.replace(/^@/, '')
  const i = auth.lastIndexOf(':')
  return i >= 0 ? { host: auth.slice(0, i), port: auth.slice(i + 1) } : { host: auth, port: '' }
}

function param(u: string, key: string) {
  return new URLSearchParams(u.split('?')[1] ?? '').get(key)
}

function withParam(u: string, key: string, value: string | null) {
  const [base, query = ''] = u.split('?')
  const params = new URLSearchParams(query)
  if (value) params.set(key, value)
  else params.delete(key)
  const q = params.toString()
  return q ? `${base}?${q}` : base
}

const unsupported = computed(() =>
  scheme.value && !props.protocols.includes(scheme.value)
    ? `${scheme.value}:// isn't available on this node`
    : null,
)

// ── SRT ───────────────────────────────────────────────────────────────────────

const srtListener = computed(
  () => scheme.value === 'srt' && (!hostPort(url.value).host || param(url.value, 'mode') === 'listener'),
)
/** The caller's host, kept while toggling to listener and back. */
const lastHost = ref('')

const srtMode = computed({
  get: () => (srtListener.value ? 'listener' : 'caller'),
  set: (mode: string | undefined) => {
    if (!mode || mode === srtMode.value) return
    const { host, port } = hostPort(url.value)
    const query = url.value.split('?')[1]
    const params = new URLSearchParams(query ?? '')
    params.delete('mode')
    const q = params.toString() ? `?${params}` : ''
    if (mode === 'listener') {
      lastHost.value = host
      cfg.value = { ...cfg.value, url: `srt://:${port || 9000}${q}` }
    } else {
      cfg.value = { ...cfg.value, url: `srt://${lastHost.value || '10.0.0.20'}:${port || 9000}${q}` }
    }
  },
})

const passphrase = computed({
  get: () => param(url.value, 'passphrase') ?? '',
  set: (v: string) => (cfg.value = { ...cfg.value, url: withParam(url.value, 'passphrase', v || null) }),
})

// ── Listener address ──────────────────────────────────────────────────────────

/** Where to send to, for a stream the node listens for. */
const pushTo = computed(() => {
  const { host, port } = hostPort(url.value)
  if (!port) return null
  if (srtListener.value) return `srt://${props.host}:${port}`
  if (scheme.value === 'udp') return `udp://${host && host !== '0.0.0.0' ? host : props.host}:${port}`
  return null
})

const TRANSPORTS: { value: RtspTransport; label: string }[] = [
  { value: 'auto', label: 'Auto (UDP, then TCP)' },
  { value: 'tcp', label: 'TCP (through firewalls)' },
  { value: 'udp', label: 'UDP' },
]

const usesLatency = computed(() => scheme.value === 'rtsp' || scheme.value === 'rtsps' || scheme.value === 'srt')
const advancedOpen = ref(false)
</script>

<template>
  <div class="col-span-2 flex flex-col gap-3">
    <FormField label="URL">
      <Input v-model="cfg.url" class="font-mono text-[0.6875rem]" placeholder="rtsp://camera.local:554/stream1" />
    </FormField>
    <div class="-mt-1.5 flex flex-wrap gap-1">
      <button
        v-for="e in examples"
        :key="e.url"
        type="button"
        class="rounded border border-border px-1.5 py-0.5 text-[11px] text-muted-foreground hover:text-foreground hover:bg-accent"
        :title="e.url"
        @click="cfg = { ...cfg, url: e.url }"
      >
        {{ e.label }}
      </button>
    </div>
    <p v-if="unsupported" class="text-xs text-destructive">{{ unsupported }}</p>

    <template v-if="scheme === 'srt'">
      <FormField label="SRT mode">
        <ToggleGroup v-model="srtMode" type="single" variant="segmented" class="w-full">
          <ToggleGroupItem value="listener" class="flex-1 h-8 text-xs">Listener (senders push here)</ToggleGroupItem>
          <ToggleGroupItem value="caller" class="flex-1 h-8 text-xs">Caller (connect out)</ToggleGroupItem>
        </ToggleGroup>
      </FormField>
      <FormField label="Passphrase (optional)">
        <Input v-model="passphrase" type="password" autocomplete="off" placeholder="10–79 characters" />
      </FormField>
    </template>

    <FormField v-if="scheme === 'rtsp' || scheme === 'rtsps'" label="Transport">
      <OptionSelect v-model="cfg.rtsp_transport" :options="TRANSPORTS" />
    </FormField>

    <div v-if="pushTo" class="rounded-md border border-border bg-muted/40 px-2.5 py-2 text-xs flex flex-col gap-1">
      <span class="text-muted-foreground">Send to</span>
      <span class="flex items-center gap-1 font-mono">
        {{ pushTo }}<template v-if="srtListener && passphrase">?passphrase=…</template>
        <CopyButton :value="pushTo" />
      </span>
      <span v-if="scheme === 'udp'" class="text-muted-foreground">MPEG-TS over UDP.</span>
      <span v-else class="text-muted-foreground">In OBS: Settings › Stream › Custom, Server set to this address.</span>
    </div>

    <AudioPlanField v-model="cfg.audio" :devices="devices" source-label="From stream" />

    <Collapsible v-model:open="advancedOpen">
      <CollapsibleTrigger class="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground">
        <ChevronRight class="size-3.5 transition-transform" :class="advancedOpen && 'rotate-90'" />
        Advanced
        <span v-if="!advancedOpen" class="opacity-70">
          — {{ usesLatency ? `${cfg.latency_ms} ms latency, ` : '' }}{{ cfg.format ? 'fixed format' : "source's format" }}
        </span>
      </CollapsibleTrigger>
      <CollapsibleContent class="flex flex-col gap-3 pt-3">
        <FormField v-if="usesLatency" label="Latency (ms)">
          <Input v-model.number="cfg.latency_ms" type="number" min="20" max="8000" />
        </FormField>
        <LiveFormatField v-model="cfg.format" />
      </CollapsibleContent>
    </Collapsible>
  </div>
</template>
