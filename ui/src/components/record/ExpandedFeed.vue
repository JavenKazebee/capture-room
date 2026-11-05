<script setup lang="ts">
import { ref, toRef } from 'vue'
import { Eye, EyeOff, Minimize2 } from '@lucide/vue'
import { useSourcesStore, type Source } from '@/stores/sources'
import { totalDropped } from '@/stores/recordings'
import { useRecordDeskStore } from '@/stores/recordDesk'
import { useNodesStore } from '@/stores/nodes'
import { usePreferences } from '@/composables/usePreferences'
import { useThumbnail } from '@/composables/useThumbnail'
import { useFeedState } from './useFeedState'
import { Button } from '@/components/ui/button'
import AudioMeter from '@/components/AudioMeter.vue'
import PresetSelect from './PresetSelect.vue'
import TallyBadge from '@/components/common/TallyBadge.vue'

/** One feed filling the Record view: its picture large, with its meters, record state and Record/Stop. */
const props = defineProps<{ source: Source }>()
const emit = defineEmits<{ close: [] }>()

const desk = useRecordDeskStore()
const sources = useSourcesStore()
const nodes = useNodesStore()
const { overlays } = usePreferences()

const { session, busy, channels, preset, duration, noSignal, format } = useFeedState(toRef(props, 'source'))
const root = ref<HTMLElement | null>(null)
const thumb = useThumbnail(toRef(props, 'source'))

defineExpose({ focus: () => root.value?.focus({ preventScroll: true }) })

// Space/Enter record or stop, as on the focused tile; on a control they press it instead.
function onKeyDown(e: KeyboardEvent) {
  if (e.target !== root.value || e.ctrlKey || e.metaKey || e.altKey || e.shiftKey) return
  if (e.key !== ' ' && e.key !== 'Enter') return
  e.preventDefault()
  // Holding the key would start then stop.
  if (!e.repeat) desk.toggle(props.source)
}
</script>

<template>
  <div
    ref="root"
    data-expanded-feed
    tabindex="0"
    :aria-label="`${source.display_name}, expanded`"
    class="h-full min-h-0 rounded-lg border bg-card overflow-hidden flex flex-col outline-none focus-visible:ring-2 focus-visible:ring-ring"
    :class="session ? 'border-tally ring-1 ring-tally' : 'border-border'"
    @keydown="onKeyDown"
  >
    <div class="flex-1 min-h-0 flex bg-black" @dblclick="emit('close')">
      <div class="flex-1 min-w-0 relative overflow-hidden">
        <div
          v-if="source.error"
          class="absolute inset-0 flex flex-col items-center justify-center gap-1 px-4 text-center text-sm"
        >
          <span class="text-destructive font-medium">Source failed</span>
          <span class="text-zinc-400">{{ source.error }}</span>
        </div>
        <div
          v-else-if="source.thumbnail_hidden"
          class="absolute inset-0 flex flex-col items-center justify-center gap-2 text-zinc-500 text-sm"
        >
          <EyeOff class="size-5" />
          Thumbnail hidden
          <button
            class="rounded px-2 py-0.5 text-zinc-300 hover:text-white hover:bg-white/10"
            @click="sources.setThumbnailHidden(source, false)"
            @dblclick.stop
          >
            Show
          </button>
        </div>
        <img
          v-else-if="thumb.src.value"
          :src="thumb.src.value"
          :alt="source.display_name"
          class="w-full h-full object-contain"
          draggable="false"
          @error="thumb.onError"
        />
        <div v-else class="absolute inset-0 grid place-items-center text-zinc-500 text-sm">No signal</div>
        <div
          v-if="noSignal"
          class="absolute inset-0 flex flex-col items-center justify-center gap-1 px-4 text-center text-sm bg-black/60"
        >
          <span :class="noSignal.dot === 'warn' ? 'text-warning font-medium' : 'text-zinc-300'">{{ noSignal.message }}</span>
          <span v-if="session" class="text-zinc-400">Recording black and silence</span>
        </div>

        <div v-if="session || noSignal" class="absolute top-2 right-2 flex flex-col items-end gap-1">
          <TallyBadge v-if="session" :duration="duration" />
          <span
            v-if="noSignal"
            class="rounded-sm px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide"
            :class="noSignal.dot === 'warn' ? 'bg-warning text-black' : 'bg-black/70 text-zinc-300'"
          >
            {{ noSignal.short }}
          </span>
          <span
            v-if="session && totalDropped(session)"
            class="rounded-sm bg-warning px-1.5 py-0.5 text-[10px] font-semibold text-black num"
            title="Video frames dropped because an encoder couldn't keep up"
          >
            {{ totalDropped(session).toLocaleString() }} dropped
          </span>
        </div>

        <div
          v-if="overlays.timecode && source.timecode"
          class="absolute bottom-2 left-2 bg-black/70 text-white text-xs num px-1.5 py-0.5 rounded"
        >
          {{ source.timecode }}
        </div>
        <div
          v-if="overlays.format && format"
          class="absolute bottom-2 right-2 bg-black/70 text-white text-xs num px-1.5 py-0.5 rounded"
        >
          {{ format }}
        </div>
      </div>

      <!-- Meters stay on here whatever the tile overlays say: this is where the feed is monitored. -->
      <div v-if="channels.length > 0" class="w-12 py-2 shrink-0">
        <AudioMeter :channels="channels" />
      </div>
    </div>

    <div class="px-3 py-2 flex items-center gap-3 min-w-0">
      <span class="text-sm font-medium truncate">{{ source.display_name }}</span>
      <span class="text-[10px] text-muted-foreground uppercase tracking-wide shrink-0">
        <template v-if="nodes.nodes.length > 1">{{ nodes.nameOf(source.node_id) }} · </template>{{ source.source_type }}
      </span>
      <p v-if="session?.error_message" class="text-xs text-destructive truncate" :title="session.error_message">
        Output failed: {{ session.error_message }}
      </p>
      <span class="flex-1" />
      <PresetSelect
        v-model="preset"
        :feeds="[source.key]"
        :options="desk.presetOptions"
        :disabled="!!session || busy"
        class="h-7 text-xs w-48 shrink-0"
      />
      <Button
        :variant="session ? 'default' : 'outline'"
        size="sm"
        class="h-7 px-3 text-xs shrink-0 gap-1.5"
        :class="session && 'bg-tally text-tally-foreground hover:bg-tally/85'"
        :disabled="busy"
        @click="desk.toggle(source)"
      >
        <span v-if="session" class="size-2 rounded-[1px] bg-current" />
        <span v-else class="size-2 rounded-full bg-tally" />
        {{ session ? 'Stop' : 'Record' }}
      </Button>
      <Button
        variant="ghost"
        size="sm"
        class="h-7 w-7 p-0 shrink-0"
        :aria-label="source.thumbnail_hidden ? 'Show thumbnail' : 'Hide thumbnail'"
        :title="source.thumbnail_hidden ? 'Show thumbnail' : 'Hide thumbnail (saves the node making it)'"
        @click="sources.setThumbnailHidden(source, !source.thumbnail_hidden)"
      >
        <Eye v-if="source.thumbnail_hidden" class="size-3.5" />
        <EyeOff v-else class="size-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="sm"
        class="h-7 w-7 p-0 shrink-0"
        aria-label="Back to all feeds"
        title="Back to all feeds (F or Esc)"
        @click="emit('close')"
      >
        <Minimize2 class="size-3.5" />
      </Button>
    </div>
  </div>
</template>
