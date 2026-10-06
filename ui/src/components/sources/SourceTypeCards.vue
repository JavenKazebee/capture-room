<script setup lang="ts">
import type { Component } from 'vue'
import { Film, Network, Palette, RadioTower, Webcam } from '@lucide/vue'
import type { ConfiguredKind } from '@/stores/sources'

/** The first step of adding a source: what kind it is. */
defineProps<{
  /** Why a kind can't be added on the node, if it can't. */
  unavailable: Partial<Record<ConfiguredKind, string>>
}>()
const emit = defineEmits<{ pick: [kind: ConfiguredKind] }>()

const CARDS: { kind: ConfiguredKind; icon: Component; label: string; hint: string }[] = [
  { kind: 'stream', icon: Network, label: 'Network stream', hint: 'RTSP cameras, SRT, RTMP, HLS, UDP' },
  { kind: 'device', icon: Webcam, label: 'Capture device', hint: 'Capture cards, webcams, screens' },
  { kind: 'whip', icon: RadioTower, label: 'WHIP ingest', hint: 'OBS or a browser pushes WebRTC' },
  { kind: 'file', icon: Film, label: 'Media file', hint: 'A file on the node, looped' },
  { kind: 'test', icon: Palette, label: 'Test pattern', hint: 'Bars, tone and other test signals' },
]
</script>

<template>
  <div class="grid grid-cols-2 gap-2">
    <button
      v-for="c in CARDS"
      :key="c.kind"
      type="button"
      class="flex flex-col items-start gap-1 rounded-lg border border-border p-3 text-left transition-colors enabled:hover:border-primary enabled:hover:bg-accent disabled:opacity-50 disabled:cursor-not-allowed"
      :disabled="!!unavailable[c.kind]"
      @click="emit('pick', c.kind)"
    >
      <span class="flex items-center gap-1.5 text-sm font-medium">
        <component :is="c.icon" class="size-4 text-primary" />
        {{ c.label }}
      </span>
      <span class="text-xs text-muted-foreground">{{ unavailable[c.kind] ?? c.hint }}</span>
    </button>
    <p class="col-span-2 text-xs text-muted-foreground">NDI sources are found by scanning; they aren't added here.</p>
  </div>
</template>
