<script setup lang="ts">
import { Flag, Trash2 } from '@lucide/vue'
import { formatDuration } from '@/lib/format'
import { markerSwatch } from '@/lib/markers'
import { notifyError } from '@/lib/notify'
import { useRecordingsStore, type RecordingSession } from '@/stores/recordings'
import { useMarkerExport } from '@/composables/useMarkerExport'
import type { MarkerDto } from '@/types/generated/MarkerDto'
import type { MarkerUpdate } from '@/types/generated/MarkerUpdate'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import MarkerColorPicker from './MarkerColorPicker.vue'

/** A recording's markers: when, a note and a colour, each editable. */
const props = defineProps<{ session: RecordingSession }>()

const recordings = useRecordingsStore()
const { exportMarkers } = useMarkerExport()

/** How far into the recording, when its first frame's time is known. */
function offset(m: MarkerDto) {
  const first = props.session.clock?.first_frame_us
  return first == null ? null : formatDuration(Math.max(0, (m.at_us - first) / 1000))
}

async function update(m: MarkerDto, change: MarkerUpdate) {
  if (change.note !== undefined && change.note === m.note) return
  try {
    await recordings.updateMarker(props.session.node_id, props.session.id, m.id, change)
  } catch (e) {
    notifyError("Couldn't save the marker", e, props.session.node_id)
  }
}

async function remove(m: MarkerDto) {
  try {
    await recordings.deleteMarker(props.session.node_id, props.session.id, m.id)
  } catch (e) {
    notifyError("Couldn't delete the marker", e, props.session.node_id)
  }
}

function saveNote(m: MarkerDto, e: Event) {
  void update(m, { note: (e.target as HTMLInputElement).value.trim() })
}
</script>

<template>
  <div v-if="session.markers.length" class="space-y-1.5">
    <div class="flex items-center gap-2">
      <h3 class="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">Markers</h3>
      <span class="num text-xs text-muted-foreground">{{ session.markers.length }}</span>
      <span class="flex-1" />
      <Button
        variant="outline"
        size="sm"
        class="h-6 gap-1.5 text-xs"
        title="Download as an EDL for Resolve: Timelines → Import → Timeline Markers from EDL"
        @click="exportMarkers([session])"
      >
        <Flag class="size-3" /> Export for Resolve
      </Button>
    </div>
    <ul class="divide-y divide-border rounded-md border border-border">
      <li v-for="m in session.markers" :key="m.id" class="flex items-center gap-2 px-2 py-1">
        <Popover>
          <PopoverTrigger as-child>
            <button
              class="size-3.5 shrink-0 rounded-full ring-offset-2 ring-offset-card hover:ring-2 hover:ring-border"
              :style="{ backgroundColor: markerSwatch(m.color) }"
              :aria-label="`Colour: ${m.color}`"
              title="Change the colour"
            />
          </PopoverTrigger>
          <PopoverContent class="w-auto p-2">
            <MarkerColorPicker :model-value="m.color" @update:model-value="(color) => update(m, { color })" />
          </PopoverContent>
        </Popover>
        <span class="num text-xs w-24 shrink-0" :title="m.timecode ? 'Timecode in the files' : 'No timecode: only MOV files carry it'">
          {{ m.timecode ?? '—' }}
        </span>
        <span class="num text-xs text-muted-foreground w-14 shrink-0" title="Into the recording">{{ offset(m) }}</span>
        <input
          :value="m.note"
          class="flex-1 min-w-0 bg-transparent text-xs outline-none rounded px-1 py-0.5 hover:bg-accent focus:bg-accent"
          placeholder="Add a note"
          aria-label="Note"
          @change="saveNote(m, $event)"
          @keydown.enter="($event.target as HTMLInputElement).blur()"
        />
        <button
          class="size-6 shrink-0 grid place-items-center rounded text-muted-foreground hover:text-destructive hover:bg-accent"
          aria-label="Delete marker"
          title="Delete marker"
          @click="remove(m)"
        >
          <Trash2 class="size-3.5" />
        </button>
      </li>
    </ul>
  </div>
</template>
