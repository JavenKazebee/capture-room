<script setup lang="ts">
import type { Component } from 'vue'
import { Cable, Film, Network, Palette, RadioTower, Webcam } from '@lucide/vue'
import type { ConfiguredKind } from '@/stores/sources'

/** The first step of adding a source: what kind it is. */
defineProps<{
  /** Why a kind can't be added on the node, if it can't. */
  unavailable: Partial<Record<ConfiguredKind, string>>
}>()
const emit = defineEmits<{ pick: [kind: ConfiguredKind] }>()

const CARDS: { kind: ConfiguredKind; icon: Component; label: string; hint: string }[] = [
  { kind: 'stream', icon: Network, label: 'Network stream', hint: 'RTSP cameras, SRT, RTMP, HLS, UDP' },
  { kind: 'device', icon: Webcam, label: 'Capture device', hint: 'Webcams, USB capture devices, screens' },
  { kind: 'decklink', icon: Cable, label: 'SDI (Decklink)', hint: 'An input on a Blackmagic Decklink card' },
  { kind: 'whip', icon: RadioTower, label: 'WHIP ingest', hint: 'OBS or a browser pushes WebRTC' },
  { kind: 'file', icon: Film, label: 'Media file', hint: 'A file on the node, looped' },
  { kind: 'test', icon: Palette, label: 'Test pattern', hint: 'Bars, tone and other test signals' },
]
</script>

<template>
  <div class="grid grid-cols-2 gap-2">
    <template v-for="c in CARDS" :key="c.kind">
      <!-- Not a disabled button: the link to Checks inside has to take clicks. -->
      <div v-if="unavailable[c.kind]" class="flex flex-col items-start gap-1 rounded-lg border border-border p-3">
        <span class="flex items-center gap-1.5 text-sm font-medium opacity-50">
          <component :is="c.icon" class="size-4 text-primary" />
          {{ c.label }}
        </span>
        <span class="text-xs text-muted-foreground">
          {{ unavailable[c.kind] }} ·
          <RouterLink to="/setup/checks" class="text-primary hover:underline">Open Checks</RouterLink>
        </span>
      </div>
      <button
        v-else
        type="button"
        class="flex flex-col items-start gap-1 rounded-lg border border-border p-3 text-left transition-colors hover:border-primary hover:bg-accent"
        @click="emit('pick', c.kind)"
      >
        <span class="flex items-center gap-1.5 text-sm font-medium">
          <component :is="c.icon" class="size-4 text-primary" />
          {{ c.label }}
        </span>
        <span class="text-xs text-muted-foreground">{{ c.hint }}</span>
      </button>
    </template>
    <p class="col-span-2 text-xs text-muted-foreground">
      NDI sources are found by scanning, and playout channels are made in Playback; neither is added here.
    </p>
  </div>
</template>
