<script setup lang="ts">
import { computed, toRef, watch } from 'vue'
import { useNow } from '@vueuse/core'
import { Check } from '@lucide/vue'
import { audioLevels, type Source } from '@/stores/sources'
import { useRecordingsStore } from '@/stores/recordings'
import { useRecordDeskStore } from '@/stores/recordDesk'
import { useNodesStore } from '@/stores/nodes'
import { useEventsStore } from '@/stores/events'
import { usePreferences } from '@/composables/usePreferences'
import { formatDuration } from '@/lib/format'
import { fpsLabel } from '@/lib/sourceFormat'
import { Button } from '@/components/ui/button'
import AudioMeter from '@/components/AudioMeter.vue'
import OptionSelect from '@/components/OptionSelect.vue'
import TallyBadge from '@/components/common/TallyBadge.vue'
import { useThumbnail } from './useThumbnail'

const props = defineProps<{ source: Source }>()

const desk = useRecordDeskStore()
const recordings = useRecordingsStore()
const nodes = useNodesStore()
const { overlays } = usePreferences()

const session = computed(() => recordings.activeForSource(props.source.node_id, props.source.id))
const selected = computed(() => desk.selected.has(props.source.key))
const busy = computed(() => desk.busy.has(props.source.key))
const channels = computed(() => audioLevels.get(props.source.key) ?? [])
const thumb = useThumbnail(toRef(props, 'source'))

const preset = computed({
  get: () => desk.presetIdOf(props.source.key),
  set: (id: string) => desk.setPreset(props.source.key, id),
})

const now = useNow({ interval: 1000 })
const duration = computed(() =>
  session.value ? formatDuration(now.value.getTime() - new Date(session.value.started_at).getTime()) : '',
)

const format = computed(() => {
  const c = props.source.capabilities
  return c ? `${c.max_height}p${fpsLabel(c.max_framerate, true)}` : null
})

function onClick(e: MouseEvent) {
  if (e.shiftKey) desk.selectRange(props.source.key)
  else desk.select(props.source.key, e.ctrlKey || e.metaKey ? 'toggle' : 'only')
}

// A session that ends on its own in error is worth a log line.
watch(session, (now, prev) => {
  if (now || !prev) return
  const ended = recordings.find(prev.node_id, prev.id)
  if (ended?.status === 'error' && ended.error_message) {
    useEventsStore().log('error', `Recording ended: ${props.source.display_name}`, {
      node_id: prev.node_id,
      detail: ended.error_message,
    })
  }
})
</script>

<template>
  <div
    class="group relative rounded-lg border bg-card overflow-hidden flex flex-col cursor-pointer select-none transition-shadow"
    :class="[
      session ? 'border-tally ring-1 ring-tally' : 'border-border hover:border-foreground/20',
      selected && 'outline-2 outline-offset-2 outline-primary',
    ]"
    @click="onClick"
  >
    <!-- Picture + meters -->
    <div class="relative flex bg-black aspect-video">
      <div class="flex-1 relative overflow-hidden">
        <div
          v-if="source.error"
          class="absolute inset-0 flex flex-col items-center justify-center gap-1 px-4 text-center text-xs"
        >
          <span class="text-destructive font-medium">Source failed</span>
          <span class="text-zinc-400 line-clamp-3">{{ source.error }}</span>
        </div>
        <img
          v-else-if="thumb.src.value"
          :src="thumb.src.value"
          :alt="source.display_name"
          class="w-full h-full object-cover"
          draggable="false"
          @error="thumb.onError"
        />
        <div v-else class="absolute inset-0 grid place-items-center text-zinc-500 text-xs">No signal</div>

        <!-- Selection checkbox (top-left) -->
        <button
          class="absolute top-1.5 left-1.5 size-5 rounded-[4px] border grid place-items-center transition-opacity"
          :class="
            selected
              ? 'bg-primary border-primary text-primary-foreground opacity-100'
              : 'bg-black/50 border-white/40 text-transparent opacity-0 group-hover:opacity-100'
          "
          :aria-label="selected ? 'Deselect' : 'Select'"
          @click.stop="desk.select(source.key, 'toggle')"
        >
          <Check class="size-3.5" />
        </button>

        <!-- Live (top-right) -->
        <div v-if="session" class="absolute top-1.5 right-1.5">
          <TallyBadge :duration="duration" />
        </div>

        <!-- Timecode (bottom-left) / format (bottom-right) -->
        <div
          v-if="overlays.timecode && source.timecode"
          class="absolute bottom-1.5 left-1.5 bg-black/70 text-white text-[10px] num px-1.5 py-0.5 rounded"
        >
          {{ source.timecode }}
        </div>
        <div
          v-if="overlays.format && format"
          class="absolute bottom-1.5 right-1.5 bg-black/70 text-white text-[10px] num px-1.5 py-0.5 rounded"
        >
          {{ format }}
        </div>
      </div>

      <div v-if="overlays.meters && channels.length > 0" class="w-8 py-1 shrink-0">
        <AudioMeter :channels="channels" />
      </div>
    </div>

    <!-- Name + controls -->
    <div class="px-2.5 py-2 flex flex-col gap-1.5">
      <div class="flex items-center justify-between gap-2 min-w-0">
        <span class="text-sm font-medium truncate">{{ source.display_name }}</span>
        <span class="text-[10px] text-muted-foreground uppercase tracking-wide shrink-0">
          <template v-if="overlays.node && nodes.nodes.length > 1">{{ nodes.nameOf(source.node_id) }} · </template>{{ source.source_type }}
        </span>
      </div>

      <div class="flex gap-1.5 items-center" @click.stop>
        <OptionSelect
          v-model="preset"
          :options="desk.presetOptions"
          :disabled="!!session || busy"
          class="h-7 text-xs flex-1 min-w-0"
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
      </div>

      <p v-if="session?.error_message" class="text-xs text-destructive break-words">
        Output failed: {{ session.error_message }}
      </p>
    </div>
  </div>
</template>
