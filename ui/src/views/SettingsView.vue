<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { nodeApi } from '@/composables/useApi'
import { usePreferences } from '@/composables/usePreferences'
import { useNodesStore } from '@/stores/nodes'
import type { MonitorSettingsDto } from '@/types/generated/MonitorSettingsDto'
import type { NodeSettingsDto } from '@/types/generated/NodeSettingsDto'
import { Button } from '@/components/ui/button'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import OptionSelect from '@/components/OptionSelect.vue'
import PageHeader from '@/components/common/PageHeader.vue'

const { colorMode, density } = usePreferences()
const nodes = useNodesStore()

// ── Monitoring (applied to every reachable node) ─────────────────────────────

/** The form's draft: this node's settings once loaded, sent to every node on Apply. */
const monitor = ref<MonitorSettingsDto | null>(null)
const monitorSaving = ref(false)
const monitorError = ref('')

watch(
  () => nodes.self?.monitor,
  (m) => {
    if (m && !monitor.value) monitor.value = { ...m }
  },
  { immediate: true },
)

const THUMB_SIZES = [
  { label: '320×180', value: '320x180' },
  { label: '640×360', value: '640x360' },
  { label: '1280×720', value: '1280x720' },
]
const THUMB_FPS = [1, 2, 5, 10].map((v) => ({ label: `${v} fps`, value: v }))
const LEVEL_INTERVALS = [50, 100, 200, 500].map((v) => ({ label: `${v} ms`, value: v }))

const thumbSize = computed({
  get: () => (monitor.value ? `${monitor.value.thumb_width}x${monitor.value.thumb_height}` : ''),
  set: (v: string) => {
    const [w, h] = v.split('x').map(Number)
    if (monitor.value) monitor.value = { ...monitor.value, thumb_width: w!, thumb_height: h! }
  },
})

/** A draft field that reads as 0 and ignores writes until settings load, so the controls render from the start. */
function field(key: 'thumb_fps' | 'level_interval_ms') {
  return computed({
    get: () => monitor.value?.[key] ?? 0,
    set: (v: number) => {
      if (monitor.value) monitor.value = { ...monitor.value, [key]: v }
    },
  })
}
const thumbFps = field('thumb_fps')
const levelInterval = field('level_interval_ms')

async function saveMonitorSettings() {
  if (!monitor.value || monitorSaving.value) return
  monitorSaving.value = true
  monitorError.value = ''
  try {
    const body = { monitor: monitor.value }
    const results = await Promise.allSettled(
      nodes.reachable.map((n) =>
        nodeApi(n.id)<NodeSettingsDto>('/settings', { method: 'PUT', body }),
      ),
    )
    const failed = results.filter((r) => r.status === 'rejected').length
    if (failed) monitorError.value = `Couldn't apply to ${failed} node${failed > 1 ? 's' : ''}.`
    const first = results.find((r) => r.status === 'fulfilled')
    if (first) {
      monitor.value = first.value.monitor
      if (nodes.self) nodes.self.monitor = first.value.monitor
    }
  } finally {
    monitorSaving.value = false
  }
}
</script>

<template>
  <PageHeader title="Settings" />
  <div class="p-4 max-w-3xl space-y-4">
    <section class="rounded-lg border border-border bg-card">
      <div class="px-4 py-2.5 border-b border-border">
        <h2 class="text-sm font-semibold">Appearance</h2>
        <p class="text-xs text-muted-foreground">Stored in this browser only.</p>
      </div>
      <div class="divide-y divide-border">
        <div class="flex items-center justify-between gap-4 px-4 py-3">
          <div>
            <div class="text-sm">Theme</div>
            <div class="text-xs text-muted-foreground">Dark is the primary theme.</div>
          </div>
          <ToggleGroup :model-value="colorMode" @update:model-value="(v) => v && (colorMode = v as typeof colorMode)" type="single" variant="outline" size="sm">
            <ToggleGroupItem value="dark">Dark</ToggleGroupItem>
            <ToggleGroupItem value="light">Light</ToggleGroupItem>
            <ToggleGroupItem value="auto">System</ToggleGroupItem>
          </ToggleGroup>
        </div>
        <div class="flex items-center justify-between gap-4 px-4 py-3">
          <div>
            <div class="text-sm">Density</div>
            <div class="text-xs text-muted-foreground">Scales text and spacing across the whole UI.</div>
          </div>
          <ToggleGroup :model-value="density" @update:model-value="(v) => v && (density = v as typeof density)" type="single" variant="outline" size="sm">
            <ToggleGroupItem value="compact">Compact</ToggleGroupItem>
            <ToggleGroupItem value="default">Default</ToggleGroupItem>
            <ToggleGroupItem value="comfortable">Comfortable</ToggleGroupItem>
          </ToggleGroup>
        </div>
      </div>
    </section>

    <section class="rounded-lg border border-border bg-card">
      <div class="px-4 py-2.5 border-b border-border">
        <h2 class="text-sm font-semibold">Monitoring</h2>
        <p class="text-xs text-muted-foreground">
          How often each source's thumbnail and audio meter update. Applied to every reachable node.
        </p>
      </div>
      <div class="flex items-center gap-2 flex-wrap px-4 py-3">
        <span class="text-xs text-muted-foreground">Thumbnail</span>
        <OptionSelect v-model="thumbSize" :options="THUMB_SIZES" :disabled="!monitor || monitorSaving" class="w-28" />
        <OptionSelect v-model="thumbFps" :options="THUMB_FPS" :disabled="!monitor || monitorSaving" class="w-20" />
        <span class="text-xs text-muted-foreground ml-2">Audio meter</span>
        <OptionSelect
          v-model="levelInterval"
          :options="LEVEL_INTERVALS"
          :disabled="!monitor || monitorSaving"
          class="w-20"
        />
        <Button size="sm" variant="outline" class="w-20" :disabled="!monitor || monitorSaving" @click="saveMonitorSettings">
          {{ monitorSaving ? 'Applying…' : 'Apply' }}
        </Button>
      </div>
      <p v-if="monitorError" class="px-4 pb-3 -mt-1 text-xs text-destructive">{{ monitorError }}</p>
    </section>
  </div>
</template>
