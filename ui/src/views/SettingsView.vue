<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { onBeforeRouteLeave } from 'vue-router'
import { toast } from 'vue-sonner'
import { AlertTriangle, RotateCcw } from '@lucide/vue'
import { nodeApi } from '@/composables/useApi'
import { usePreferences } from '@/composables/usePreferences'
import { formatUptime } from '@/lib/format'
import { notifyError } from '@/lib/notify'
import { useNodesStore } from '@/stores/nodes'
import type { MonitorSettingsDto } from '@/types/generated/MonitorSettingsDto'
import type { NodeSettingsDto } from '@/types/generated/NodeSettingsDto'
import { Button } from '@/components/ui/button'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import OptionSelect from '@/components/OptionSelect.vue'
import PageHeader from '@/components/common/PageHeader.vue'
import KeyValueList from '@/components/common/KeyValueList.vue'

const { colorMode, density } = usePreferences()
const nodes = useNodesStore()

// ── Monitoring (one setting for every node) ──────────────────────────────────

/** Each reachable node's current settings; `null` = couldn't read them. */
const nodeMonitors = reactive(new Map<string, MonitorSettingsDto | null>())

async function loadNodeMonitors() {
  const ids = nodes.reachable.map((n) => n.id)
  for (const id of nodeMonitors.keys()) if (!ids.includes(id)) nodeMonitors.delete(id)
  await Promise.all(
    ids.map(async (id) => {
      const s = await nodeApi(id)<NodeSettingsDto>('/settings').catch(() => null)
      nodeMonitors.set(id, s?.monitor ?? null)
    }),
  )
}

// `nodes.load()` replaces the list on every node event, including another
// client changing settings, so this keeps the drift check current.
watch(() => nodes.nodes, loadNodeMonitors, { immediate: true })

/** What this node uses: the baseline the draft is compared to. */
const current = computed(() => nodes.self?.monitor ?? null)
const draft = ref<MonitorSettingsDto | null>(null)
const saving = ref(false)

const same = (a: MonitorSettingsDto | null | undefined, b: MonitorSettingsDto | null | undefined) =>
  !!a && !!b &&
  a.thumb_width === b.thumb_width &&
  a.thumb_height === b.thumb_height &&
  a.thumb_fps === b.thumb_fps &&
  a.level_interval_ms === b.level_interval_ms

const dirty = computed(() => !!draft.value && !!current.value && !same(draft.value, current.value))

// Follow the server until the user edits; then keep their draft.
watch(current, (m) => {
  if (m && (!draft.value || !dirty.value)) draft.value = { ...m }
}, { immediate: true })

/** Reachable nodes whose settings differ from this one's. */
const drifted = computed(() =>
  nodes.reachable.filter((n) => {
    const m = nodeMonitors.get(n.id)
    return m && current.value && !same(m, current.value)
  }),
)
const unreachable = computed(() => nodes.nodes.filter((n) => !n.healthy))

function revert() {
  if (current.value) draft.value = { ...current.value }
}

async function apply() {
  if (!draft.value || saving.value) return
  saving.value = true
  const monitor = draft.value
  const targets = nodes.reachable
  try {
    const results = await Promise.allSettled(
      targets.map((n) => nodeApi(n.id)<NodeSettingsDto>('/settings', { method: 'PUT', body: { monitor } })),
    )
    results.forEach((r, i) => {
      if (r.status === 'fulfilled') nodeMonitors.set(targets[i]!.id, r.value.monitor)
    })
    const selfResult = results.find((r, i) => r.status === 'fulfilled' && targets[i]!.is_self)
    if (selfResult?.status === 'fulfilled' && nodes.self) nodes.self.monitor = selfResult.value.monitor
    // The server clamps values; show what it actually stored.
    if (current.value) draft.value = { ...current.value }

    const failed = targets.filter((_, i) => results[i]!.status === 'rejected')
    if (!failed.length) {
      toast.success(targets.length > 1 ? `Monitoring applied to ${targets.length} nodes` : 'Monitoring applied')
    } else {
      const first = results.find((r) => r.status === 'rejected') as PromiseRejectedResult
      notifyError(`Couldn't apply monitoring to ${failed.map((n) => n.name).join(', ')}`, first.reason)
    }
  } finally {
    saving.value = false
  }
}

onBeforeRouteLeave(() => !dirty.value || confirm('Discard unapplied monitoring changes?'))

/** The fixed choices, plus the current value if it was set to something else (e.g. via the API). */
function withValue<T extends string | number>(opts: { value: T; label: string }[], value: T | undefined, label: (v: T) => string) {
  return value === undefined || opts.some((o) => o.value === value) ? opts : [...opts, { value, label: label(value) }]
}

const thumbSize = computed({
  get: () => (draft.value ? `${draft.value.thumb_width}x${draft.value.thumb_height}` : ''),
  set: (v: string) => {
    const [w, h] = v.split('x').map(Number)
    if (draft.value) draft.value = { ...draft.value, thumb_width: w!, thumb_height: h! }
  },
})

/** A draft field that reads as 0 until settings load, so the controls render from the start. */
function field(key: 'thumb_fps' | 'level_interval_ms') {
  return computed({
    get: () => draft.value?.[key] ?? 0,
    set: (v: number) => {
      if (draft.value) draft.value = { ...draft.value, [key]: v }
    },
  })
}
const thumbFps = field('thumb_fps')
const levelInterval = field('level_interval_ms')

const sizeLabel = (v: string) => v.replace('x', '×')
const thumbSizes = computed(() =>
  withValue(['320x180', '640x360', '1280x720'].map((v) => ({ value: v, label: sizeLabel(v) })), thumbSize.value || undefined, sizeLabel),
)
const fpsLabel = (v: number) => `${v} fps`
const thumbRates = computed(() => withValue([1, 2, 5, 10].map((v) => ({ value: v, label: fpsLabel(v) })), draft.value?.thumb_fps, fpsLabel))
const msLabel = (v: number) => `${v} ms`
const meterRates = computed(() =>
  withValue([50, 100, 200, 500].map((v) => ({ value: v, label: msLabel(v) })), draft.value?.level_interval_ms, msLabel),
)

// ── About ─────────────────────────────────────────────────────────────────────

const selfNode = computed(() => nodes.nodes.find((n) => n.is_self))
const peerCount = computed(() => nodes.nodes.length - 1)

const about = computed(() => [
  { label: 'Version', value: selfNode.value ? `v${selfNode.value.version}` : null, mono: true },
  { label: 'Node name', value: nodes.self?.node_name },
  { label: 'Node ID', value: nodes.self?.node_id, mono: true, copy: true },
  { label: 'API', value: `${window.location.origin}/api/v1`, mono: true, copy: true },
  {
    label: 'Controller',
    value: !nodes.self
      ? null
      : nodes.isController
        ? `On · ${peerCount.value} other node${peerCount.value === 1 ? '' : 's'}`
        : 'Off',
  },
  { label: 'Uptime', value: selfNode.value ? formatUptime(selfNode.value.uptime_secs) : null, mono: true },
])
</script>

<template>
  <PageHeader title="Settings" />
  <div class="p-4 max-w-3xl space-y-6">
    <section class="space-y-2">
      <div>
        <h2 class="text-base font-semibold tracking-tight">Appearance</h2>
        <p class="text-xs text-muted-foreground">Stored in this browser only.</p>
      </div>
      <div class="rounded-lg border border-border bg-card overflow-hidden">
        <div class="divide-y divide-border">
          <div class="flex items-center justify-between gap-4 px-4 py-3">
            <div>
              <div class="text-sm">Theme</div>
              <div class="text-xs text-muted-foreground">Dark is the primary theme.</div>
            </div>
            <ToggleGroup :model-value="colorMode" @update:model-value="(v) => v && (colorMode = v as typeof colorMode)" type="single" variant="segmented">
              <ToggleGroupItem value="dark" class="px-2.5">Dark</ToggleGroupItem>
              <ToggleGroupItem value="light" class="px-2.5">Light</ToggleGroupItem>
              <ToggleGroupItem value="auto" class="px-2.5">System</ToggleGroupItem>
            </ToggleGroup>
          </div>
          <div class="flex items-center justify-between gap-4 px-4 py-3">
            <div>
              <div class="text-sm">Density</div>
              <div class="text-xs text-muted-foreground">Scales text and spacing across the whole UI.</div>
            </div>
            <ToggleGroup :model-value="density" @update:model-value="(v) => v && (density = v as typeof density)" type="single" variant="segmented">
              <ToggleGroupItem value="compact" class="px-2.5">Compact</ToggleGroupItem>
              <ToggleGroupItem value="default" class="px-2.5">Default</ToggleGroupItem>
              <ToggleGroupItem value="comfortable" class="px-2.5">Comfortable</ToggleGroupItem>
            </ToggleGroup>
          </div>
        </div>
      </div>
    </section>

    <section class="space-y-2">
      <div>
        <h2 class="text-base font-semibold tracking-tight">Monitoring</h2>
        <p class="text-xs text-muted-foreground">How each source's thumbnail and audio meter are sampled for the Record view. Shared by every node.</p>
      </div>
      <div class="rounded-lg border border-border bg-card overflow-hidden">
        <div class="divide-y divide-border">
          <div class="flex items-center justify-between gap-4 px-4 py-3">
            <div>
              <div class="text-sm">Thumbnail size</div>
              <div class="text-xs text-muted-foreground">Larger is sharper on big tiles, but costs CPU and bandwidth per feed.</div>
            </div>
            <OptionSelect v-model="thumbSize" :options="thumbSizes" :disabled="!draft || saving" class="w-32 shrink-0" />
          </div>
          <div class="flex items-center justify-between gap-4 px-4 py-3">
            <div>
              <div class="text-sm">Thumbnail rate</div>
              <div class="text-xs text-muted-foreground">Frames per second for each tile. Recording quality is unaffected.</div>
            </div>
            <OptionSelect v-model="thumbFps" :options="thumbRates" :disabled="!draft || saving" class="w-32 shrink-0" />
          </div>
          <div class="flex items-center justify-between gap-4 px-4 py-3">
            <div>
              <div class="text-sm">Audio meter interval</div>
              <div class="text-xs text-muted-foreground">Shorter makes meters smoother and sends more updates.</div>
            </div>
            <OptionSelect v-model="levelInterval" :options="meterRates" :disabled="!draft || saving" class="w-32 shrink-0" />
          </div>
        </div>
        <div class="flex items-center gap-3 px-4 py-2.5 border-t border-border bg-muted/30">
          <!-- One line whose text changes, so the footer never grows. -->
          <p class="flex-1 min-w-0 truncate text-xs">
            <span v-if="drifted.length" class="inline-flex items-center gap-1.5 text-warning" :title="drifted.map((n) => n.name).join(', ')">
              <AlertTriangle class="size-3.5 shrink-0" />
              {{ drifted.length === 1 ? `${drifted[0]!.name} uses` : `${drifted.length} nodes use` }}
              different settings. Apply to bring {{ drifted.length === 1 ? 'it' : 'them' }} in line.
            </span>
            <span v-else class="text-muted-foreground">
              Applies to {{ nodes.reachable.length === 1 ? 'this node' : `all ${nodes.reachable.length} reachable nodes` }}<template v-if="unreachable.length">;
                {{ unreachable.length }} unreachable won't be updated</template>.
            </span>
          </p>
          <Button variant="outline" size="sm" class="h-7 gap-1.5 text-xs" :disabled="!dirty || saving" @click="revert">
            <RotateCcw class="size-3.5" /> Revert
          </Button>
          <Button size="sm" class="h-7 w-20 text-xs" :disabled="(!dirty && !drifted.length) || saving" @click="apply">
            {{ saving ? 'Applying…' : 'Apply' }}
          </Button>
        </div>
      </div>
    </section>

    <section class="space-y-2">
      <div>
        <h2 class="text-base font-semibold tracking-tight">About</h2>
        <p class="text-xs text-muted-foreground">The Capture Room instance this browser is connected to.</p>
      </div>
      <div class="rounded-lg border border-border bg-card overflow-hidden">
        <div class="px-4 py-3">
          <KeyValueList :items="about" />
        </div>
      </div>
    </section>
  </div>
</template>
