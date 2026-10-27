<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { toast } from 'vue-sonner'
import { AlertTriangle } from '@lucide/vue'
import { notifyError } from '@/lib/notify'
import { useNodesStore } from '@/stores/nodes'
import { useSettingsStore } from '@/stores/settings'
import type { ClockMode } from '@/types/generated/ClockMode'
import { Input } from '@/components/ui/input'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import LoadError from '@/components/common/LoadError.vue'

/** The controller's clock: its own, or a PTP grandmaster. Shown only on a controller. */
const nodes = useNodesStore()
const settings = useSettingsStore()

const clockSettings = computed(() => settings.clock)
const clockError = computed(() => (nodes.isController ? settings.clockError : null))
const ptpDomain = ref(0)
const savingClock = ref(false)

const loadClock = () => settings.loadClock()
// `nodes.self`, which says whether this is a controller, arrives after the node list.
watch([() => nodes.nodes, () => nodes.isController], loadClock, { immediate: true })
watch(clockSettings, (c) => {
  if (c?.mode.kind === 'ptp') ptpDomain.value = c.mode.domain
}, { immediate: true })

async function setClockMode(mode: ClockMode) {
  savingClock.value = true
  try {
    await settings.setClockMode(mode)
    toast.success('Clock mode changed; nodes switch once the new clock syncs')
  } catch (e) {
    notifyError("Couldn't change the clock mode", e)
  } finally {
    savingClock.value = false
  }
}

function onClockKind(kind: unknown) {
  if (kind === 'controller') setClockMode({ kind: 'controller' })
  else if (kind === 'ptp') setClockMode({ kind: 'ptp', domain: ptpDomain.value })
}

function commitDomain() {
  const domain = Math.min(127, Math.max(0, Math.round(Number(ptpDomain.value) || 0)))
  ptpDomain.value = domain
  const mode = clockSettings.value?.mode
  if (mode?.kind === 'ptp' && mode.domain !== domain) setClockMode({ kind: 'ptp', domain })
}
</script>

<template>
  <section v-if="clockSettings || clockError" id="clock-sync" class="space-y-2 scroll-mt-4">
    <div>
      <h2 class="text-base font-semibold tracking-tight">Clock sync</h2>
      <p class="text-xs text-muted-foreground">
        What this controller's nodes run on, so their timestamps agree. Each node shows its sync on the Nodes page.
      </p>
    </div>
    <LoadError :error="clockError" :retry="loadClock" />
    <div v-if="clockSettings" class="rounded-lg border border-border bg-card overflow-hidden">
      <div class="divide-y divide-border">
        <div class="flex items-center justify-between gap-4 px-4 py-3">
          <div>
            <div class="text-sm">Clock master</div>
            <div class="text-xs text-muted-foreground">
              This controller's clock needs no setup. PTP follows a grandmaster already on the network
              (e.g. a Dante or AES67 system) and is tighter, but needs the PTP helper's permissions on each node.
            </div>
          </div>
          <ToggleGroup
            :model-value="clockSettings.mode.kind"
            type="single"
            variant="segmented"
            :disabled="savingClock"
            @update:model-value="onClockKind"
          >
            <ToggleGroupItem value="controller" class="px-2.5">This controller</ToggleGroupItem>
            <ToggleGroupItem value="ptp" class="px-2.5">PTP</ToggleGroupItem>
          </ToggleGroup>
        </div>
        <div v-if="clockSettings.mode.kind === 'ptp'" class="flex items-center justify-between gap-4 px-4 py-3">
          <div>
            <div class="text-sm">PTP domain</div>
            <div class="text-xs text-muted-foreground">The grandmaster's domain number, 0–127. Dante and AES67 usually use 0.</div>
          </div>
          <Input
            v-model.number="ptpDomain"
            type="number"
            min="0"
            max="127"
            class="w-20 h-8 shrink-0 num"
            :disabled="savingClock"
            @blur="commitDomain"
            @keydown.enter="commitDomain"
          />
        </div>
      </div>
      <div class="flex items-center gap-3 px-4 py-2.5 border-t border-border bg-muted/30">
        <p class="flex-1 min-w-0 truncate text-xs">
          <span v-if="!clockSettings.serving" class="inline-flex items-center gap-1.5 text-warning">
            <AlertTriangle class="size-3.5 shrink-0" />
            Can't serve the clock on UDP port <span class="num">{{ clockSettings.port }}</span>; is another program using it?
          </span>
          <span v-else class="text-muted-foreground">
            Clock served on UDP port <span class="num">{{ clockSettings.port }}</span>. Nodes need to reach it through their firewalls.
          </span>
        </p>
      </div>
    </div>
  </section>
</template>
