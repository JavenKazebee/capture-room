<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRecordDeskStore } from '@/stores/recordDesk'
import { useRecordingsStore } from '@/stores/recordings'
import { notifyError } from '@/lib/notify'
import { plural } from '@/lib/format'
import type { MarkerColor } from '@/types/generated/MarkerColor'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import MarkerColorPicker from '@/components/recordings/MarkerColorPicker.vue'

/** A note and colour for every marker of one keypress (`desk.notePress`). */
const desk = useRecordDeskStore()
const recordings = useRecordingsStore()

const open = computed({
  get: () => desk.notePress !== null,
  set: (v) => {
    if (!v) desk.notePress = null
  },
})
const markers = computed(() => (desk.notePress ? recordings.pressMarkers(desk.notePress) : []))
const timecode = computed(() => markers.value.find((m) => m.marker.timecode)?.marker.timecode ?? null)

const note = ref('')
const color = ref<MarkerColor>('blue')
const saving = ref(false)

watch(
  () => desk.notePress,
  () => {
    const first = markers.value[0]?.marker
    note.value = first?.note ?? ''
    color.value = first?.color ?? 'blue'
  },
)

async function save() {
  const press = desk.notePress
  if (!press || saving.value) return
  saving.value = true
  try {
    await recordings.updatePress(press, { note: note.value.trim(), color: color.value })
    desk.notePress = null
  } catch (e) {
    notifyError("Couldn't save the note", e)
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <Dialog v-model:open="open">
    <!-- Opened from a toast's button: the toast closing must not count as a click outside. -->
    <DialogContent
      class="sm:max-w-md"
      @interact-outside="(e: Event) => (e.target as Element | null)?.closest?.('[data-sonner-toaster]') && e.preventDefault()"
    >
      <DialogHeader>
        <DialogTitle>Marker note</DialogTitle>
        <DialogDescription>
          <span v-if="timecode" class="num">{{ timecode }}</span>
          <template v-if="timecode"> · </template>On {{ plural(markers.length, 'recording') }}
        </DialogDescription>
      </DialogHeader>
      <form class="flex flex-col gap-4" @submit.prevent="save">
        <Input v-model="note" placeholder="e.g. Cut to wide" aria-label="Note" autofocus />
        <MarkerColorPicker v-model="color" />
        <DialogFooter>
          <Button type="button" variant="outline" @click="open = false">Cancel</Button>
          <Button type="submit" :disabled="saving">Save</Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
</template>
