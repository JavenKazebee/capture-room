<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { toast } from 'vue-sonner'
import { AlertTriangle, RotateCcw } from '@lucide/vue'
import { useUnsavedGuard } from '@/composables/useUnsavedGuard'
import { notifyError } from '@/lib/notify'
import { useNodesStore } from '@/stores/nodes'
import { useSettingsStore } from '@/stores/settings'
import type { MonitorSettingsDto } from '@/types/generated/MonitorSettingsDto'
import { Button } from '@/components/ui/button'
import OptionSelect from '@/components/OptionSelect.vue'
import LoadError from '@/components/common/LoadError.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'

/** How each node samples thumbnails and meters for Record, applied to every node at once. */
const nodes = useNodesStore()
const settings = useSettingsStore()

/** Without this node's settings there's nothing to edit; without a peer's, its drift is unknown. */
const monitorError = computed(
  () => (!nodes.self && nodes.error) || settings.monitorErrors.message('settings', nodes.nameOf),
)
const retryMonitoring = () => (nodes.self ? settings.loadMonitors() : nodes.load())

// `nodes.load()` replaces the list on every node event, including another
// client changing settings, so this keeps the drift check current.
watch(() => nodes.nodes, settings.loadMonitors, { immediate: true })

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
    const m = settings.monitorFor(n.id)
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
  try {
    const { applied, failed } = await settings.applyMonitor(draft.value)
    // The server clamps values; show what it actually stored.
    if (current.value) draft.value = { ...current.value }
    if (!failed.length) {
      toast.success(applied > 1 ? `Monitoring applied to ${applied} nodes` : 'Monitoring applied')
    } else {
      notifyError(`Couldn't apply monitoring to ${failed.map((f) => f.name).join(', ')}`, failed[0]!.error)
    }
  } finally {
    saving.value = false
  }
}

const { confirmOpen: confirmLeave, discard: discardAndLeave } = useUnsavedGuard(dirty)

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
const perSecond = (v: number) => `${v} fps`
const thumbRates = computed(() => withValue([1, 2, 5, 10, 15, 25, 30].map((v) => ({ value: v, label: perSecond(v) })), draft.value?.thumb_fps, perSecond))
const msLabel = (v: number) => `${v} ms`
const meterRates = computed(() =>
  withValue([50, 100, 200, 500].map((v) => ({ value: v, label: msLabel(v) })), draft.value?.level_interval_ms, msLabel),
)
</script>

<template>
  <section id="monitoring" class="space-y-2 scroll-mt-4">
    <div>
      <h2 class="text-base font-semibold tracking-tight">Monitoring</h2>
      <p class="text-xs text-muted-foreground">How each source's thumbnail and audio meter are sampled for the Record view. Applies to every node this controller manages.</p>
    </div>
    <LoadError :error="monitorError" :retry="retryMonitoring" />
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

    <ConfirmDialog
      v-model:open="confirmLeave"
      title="Discard unapplied changes?"
      description="Your monitoring changes haven't been applied."
      confirm-label="Discard"
      @confirm="discardAndLeave"
    />
  </section>
</template>
