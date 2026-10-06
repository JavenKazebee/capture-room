<script setup lang="ts">
import { computed, ref } from 'vue'
import { Pause, Play, Square } from '@lucide/vue'
import { formatClock } from '@/lib/format'
import type { ChannelStatusDto } from '@/types/generated/ChannelStatusDto'
import { Button } from '@/components/ui/button'

/**
 * Play, pause and stop, the clip's times, and a scrubber over the whole file
 * with the clip's in–out range marked. Dragging previews; release seeks.
 */
const props = defineProps<{ status?: ChannelStatusDto; busy: boolean }>()
const emit = defineEmits<{ play: []; pause: []; stop: []; seek: [ms: number] }>()

const clip = computed(() => props.status?.clip ?? null)
const duration = computed(() => props.status?.duration_ms ?? 0)
const playing = computed(() => props.status?.state === 'playing')
const canPlay = computed(() => !!clip.value && !playing.value)

const dragging = ref<number | null>(null)
const position = computed(() => dragging.value ?? props.status?.position_ms ?? 0)
const inMs = computed(() => clip.value?.in_ms ?? 0)
const outMs = computed(() => clip.value?.out_ms ?? duration.value)
const elapsed = computed(() => Math.max(0, position.value - inMs.value))
const remaining = computed(() => Math.max(0, outMs.value - position.value))
const pct = (ms: number) => (duration.value ? `${(Math.min(ms, duration.value) / duration.value) * 100}%` : '0%')

const bar = ref<HTMLElement | null>(null)
function at(e: PointerEvent) {
  const r = bar.value!.getBoundingClientRect()
  const x = Math.min(Math.max(e.clientX - r.left, 0), r.width)
  return Math.round((x / r.width) * duration.value)
}
function down(e: PointerEvent) {
  if (!clip.value || !duration.value) return
  bar.value!.setPointerCapture(e.pointerId)
  dragging.value = at(e)
}
function move(e: PointerEvent) {
  if (dragging.value !== null) dragging.value = at(e)
}
function up() {
  if (dragging.value === null) return
  emit('seek', dragging.value)
  dragging.value = null
}
</script>

<template>
  <div class="flex flex-col gap-2">
    <div class="flex items-center gap-2">
      <Button
        v-if="!playing"
        size="sm"
        class="h-8 w-24 gap-1.5"
        :disabled="!canPlay || busy"
        title="Play (Space)"
        @click="emit('play')"
      >
        <Play class="size-3.5 fill-current" /> Play
      </Button>
      <Button v-else size="sm" variant="secondary" class="h-8 w-24 gap-1.5" :disabled="busy" title="Pause (Space)" @click="emit('pause')">
        <Pause class="size-3.5 fill-current" /> Pause
      </Button>
      <Button
        size="sm"
        variant="outline"
        class="h-8 gap-1.5"
        :disabled="!clip || busy"
        title="Stop: unload the clip, back to black (Esc)"
        @click="emit('stop')"
      >
        <Square class="size-3 fill-current" /> Stop
      </Button>

      <div class="flex-1" />

      <div class="flex items-baseline gap-4 num">
        <span class="flex flex-col items-end leading-tight">
          <span class="text-[10px] uppercase tracking-wide text-muted-foreground font-sans">Elapsed</span>
          <span class="text-lg">{{ clip ? formatClock(elapsed) : '–:––.–' }}</span>
        </span>
        <span class="flex flex-col items-end leading-tight">
          <span class="text-[10px] uppercase tracking-wide text-muted-foreground font-sans">Remaining</span>
          <span class="text-lg" :class="clip && playing && remaining < 10_000 && 'text-warning'">
            {{ clip ? `−${formatClock(remaining)}` : '–:––.–' }}
          </span>
        </span>
      </div>
    </div>

    <div
      ref="bar"
      class="relative h-7 rounded bg-muted touch-none"
      :class="clip && duration ? 'cursor-pointer' : 'opacity-50'"
      role="slider"
      aria-label="Position"
      :aria-valuemin="0"
      :aria-valuemax="duration"
      :aria-valuenow="position"
      @pointerdown="down"
      @pointermove="move"
      @pointerup="up"
      @pointercancel="dragging = null"
    >
      <!-- In–out range -->
      <div
        v-if="clip"
        class="absolute inset-y-0 bg-primary/20 border-x border-primary"
        :style="{ left: pct(inMs), width: `calc(${pct(outMs)} - ${pct(inMs)})` }"
      />
      <!-- Playhead -->
      <div v-if="clip" class="absolute inset-y-0 w-0.5 -ml-px bg-foreground" :style="{ left: pct(position) }" />
      <span class="absolute left-1.5 bottom-0.5 text-[10px] num text-muted-foreground pointer-events-none">
        {{ clip ? formatClock(position) : '' }}
      </span>
      <span class="absolute right-1.5 bottom-0.5 text-[10px] num text-muted-foreground pointer-events-none">
        {{ duration ? formatClock(duration) : '' }}
      </span>
    </div>
  </div>
</template>
