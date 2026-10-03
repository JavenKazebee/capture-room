<script setup lang="ts">
import { computed, watch } from 'vue'
import { useEventListener, useStorage } from '@vueuse/core'
import { ChevronDown, ChevronUp } from '@lucide/vue'
import { useEventsStore } from '@/stores/events'
import { useRecordingsStore } from '@/stores/recordings'
import EventLog from './EventLog.vue'
import RecordingsTab from './RecordingsTab.vue'
import StorageTab from './StorageTab.vue'

type Tab = 'log' | 'recordings' | 'storage'

const open = useStorage('cr.panel.open', false)
const tab = useStorage<Tab>('cr.panel.tab', 'log')
const height = useStorage('cr.panel.height', 260)

const events = useEventsStore()
const recordings = useRecordingsStore()

const tabs = computed(() => [
  { id: 'log' as const, label: 'Log', badge: events.unseenErrors || null, badgeClass: 'bg-destructive/20 text-destructive' },
  { id: 'recordings' as const, label: 'Recordings', badge: recordings.activeSessions.length || null, badgeClass: 'bg-tally text-tally-foreground' },
  { id: 'storage' as const, label: 'Storage', badge: null, badgeClass: '' },
])

/** Clicking the active tab of an open panel closes it; any other tab opens to it. */
function select(id: Tab) {
  if (open.value && tab.value === id) open.value = false
  else {
    tab.value = id
    open.value = true
  }
}

function toggle() {
  open.value = !open.value
}
defineExpose({ toggle })

useEventListener('keydown', (e: KeyboardEvent) => {
  if (e.key.toLowerCase() === 'j' && (e.metaKey || e.ctrlKey)) {
    e.preventDefault()
    toggle()
  }
})

// Errors count as seen once the log is on screen.
watch(
  () => [open.value && tab.value === 'log', events.unseenErrors] as const,
  ([visible, n]) => visible && n && events.markSeen(),
  { immediate: true },
)

// ── Resize ────────────────────────────────────────────────────────────────────

const MIN = 120
const clamp = (h: number) => Math.round(Math.min(Math.max(h, MIN), window.innerHeight * 0.75))

function startResize(e: PointerEvent) {
  const startY = e.clientY
  const startH = height.value
  const el = e.currentTarget as HTMLElement
  el.setPointerCapture(e.pointerId)
  const move = (ev: PointerEvent) => (height.value = clamp(startH + startY - ev.clientY))
  const up = () => {
    el.removeEventListener('pointermove', move)
    el.removeEventListener('pointerup', up)
  }
  el.addEventListener('pointermove', move)
  el.addEventListener('pointerup', up)
}
</script>

<template>
  <section class="shrink-0 flex flex-col border-t border-border bg-card">
    <!-- Resize handle -->
    <div
      v-if="open"
      class="h-1 -mt-0.5 cursor-row-resize hover:bg-primary/50 active:bg-primary transition-colors"
      @pointerdown="startResize"
      @dblclick="height = 260"
    />

    <!-- Tab strip (always visible) -->
    <div class="h-8 shrink-0 flex items-stretch px-1 text-xs" :class="open && 'border-b border-border'">
      <button
        v-for="t in tabs"
        :key="t.id"
        class="relative flex items-center gap-1.5 px-3 text-muted-foreground hover:text-foreground"
        :class="open && tab === t.id && 'text-foreground font-medium after:absolute after:inset-x-2 after:top-0 after:h-0.5 after:rounded-full after:bg-primary'"
        @click="select(t.id)"
      >
        {{ t.label }}
        <span v-if="t.badge" class="num rounded-sm px-1 text-[10px] font-semibold" :class="t.badgeClass">{{ t.badge }}</span>
      </button>
      <div class="flex-1" />
      <button
        class="flex items-center gap-1.5 px-2 text-muted-foreground hover:text-foreground"
        :title="open ? 'Close panel' : 'Open panel'"
        @click="toggle"
      >
        <span class="opacity-60">Ctrl J</span>
        <ChevronDown v-if="open" class="size-3.5" />
        <ChevronUp v-else class="size-3.5" />
      </button>
    </div>

    <!-- Content -->
    <div v-if="open" class="min-h-0" :style="{ height: `${clamp(height)}px` }">
      <EventLog v-if="tab === 'log'" />
      <RecordingsTab v-else-if="tab === 'recordings'" />
      <StorageTab v-else />
    </div>
  </section>
</template>
