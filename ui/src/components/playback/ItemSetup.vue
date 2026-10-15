<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { ArrowDownToLine, Play } from '@lucide/vue'
import { formatClock, parseClock } from '@/lib/format'
import type { ChannelStatusDto } from '@/types/generated/ChannelStatusDto'
import type { ClipEnd } from '@/types/generated/ClipEnd'
import type { PlaylistItemDto } from '@/types/generated/PlaylistItemDto'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import FormField from '@/components/FormField.vue'

/**
 * A playlist item's in and out points and what happens at its out point,
 * edited here and applied to the playlist; and cueing or playing it.
 */
const props = defineProps<{
  item: PlaylistItemDto
  status: ChannelStatusDto | undefined
  busy: boolean
}>()
const emit = defineEmits<{
  apply: [setup: { in_ms: number; out_ms: number | null; end: ClipEnd }]
  cue: []
  play: []
}>()

const inText = ref('')
const outText = ref('')
const end = ref<ClipEnd>('next')

/** The item changing (another picked, or saved) resets the setup to it. */
watch(
  () => props.item,
  (i) => {
    inText.value = i.in_ms ? formatClock(i.in_ms) : ''
    outText.value = i.out_ms != null ? formatClock(i.out_ms) : ''
    end.value = i.end
  },
  { immediate: true },
)

const onAir = computed(() => props.status?.item_id === props.item.id)
const inMs = computed(() => (inText.value.trim() ? parseClock(inText.value) : 0))
const outMs = computed(() => (outText.value.trim() ? parseClock(outText.value) : null))
const error = computed(() => {
  if (inMs.value === null) return 'The in point should look like 1:23.4 or 83.4.'
  if (outText.value.trim() && outMs.value === null) return 'The out point should look like 1:23.4 or 83.4.'
  if (outMs.value !== null && outMs.value <= inMs.value) return 'The out point must be after the in point.'
  const d = props.item.duration_ms
  if (d != null && inMs.value >= d) return `The in point is past the end (${formatClock(d)}).`
  return null
})

/** The setup differs from the saved item. */
const changed = computed(
  () => props.item.in_ms !== inMs.value || props.item.out_ms !== outMs.value || props.item.end !== end.value,
)
/** On air, new in and out points only apply once it's played or cued again. */
const onAirStale = computed(() => {
  const l = props.status?.clip
  return onAir.value && !!l && (l.in_ms !== props.item.in_ms || l.out_ms !== props.item.out_ms)
})

function apply() {
  if (error.value || !changed.value || inMs.value === null) return
  emit('apply', { in_ms: inMs.value, out_ms: outMs.value, end: end.value })
}

/** Set the in or out point at the playhead, while this item is on air (I / O). */
function setAtPlayhead(which: 'in' | 'out') {
  const p = props.status?.position_ms
  if (p == null || !onAir.value) return
  if (which === 'in') inText.value = formatClock(p)
  else outText.value = formatClock(p)
}

defineExpose({ setAtPlayhead })
</script>

<template>
  <section class="rounded-lg border border-border p-3 flex flex-col gap-3">
    <div class="flex items-center gap-2 min-w-0">
      <span class="text-xs text-muted-foreground shrink-0">Item</span>
      <span class="text-sm font-medium truncate">{{ item.name }}</span>
      <span v-if="item.duration_ms != null" class="num text-xs text-muted-foreground shrink-0">
        {{ formatClock(item.duration_ms) }}
      </span>
    </div>
    <div class="flex flex-wrap items-end gap-3">
      <FormField label="In">
        <div class="flex gap-1">
          <Input v-model="inText" class="w-24 num h-8" placeholder="0:00.0" @keydown.enter="apply" />
          <Button
            variant="outline"
            size="icon"
            class="size-8"
            :disabled="!onAir"
            title="Set at the playhead (I)"
            aria-label="Set the in point at the playhead"
            @click="setAtPlayhead('in')"
          >
            <ArrowDownToLine class="size-3.5" />
          </Button>
        </div>
      </FormField>
      <FormField label="Out">
        <div class="flex gap-1">
          <Input v-model="outText" class="w-24 num h-8" placeholder="End" @keydown.enter="apply" />
          <Button
            variant="outline"
            size="icon"
            class="size-8"
            :disabled="!onAir"
            title="Set at the playhead (O)"
            aria-label="Set the out point at the playhead"
            @click="setAtPlayhead('out')"
          >
            <ArrowDownToLine class="size-3.5" />
          </Button>
        </div>
      </FormField>
      <FormField label="At the out point">
        <ToggleGroup v-model="end" type="single" variant="segmented">
          <ToggleGroupItem value="next" class="h-8 px-3 text-xs">Next</ToggleGroupItem>
          <ToggleGroupItem value="hold" class="h-8 px-3 text-xs">Hold</ToggleGroupItem>
          <ToggleGroupItem value="black" class="h-8 px-3 text-xs">Black</ToggleGroupItem>
          <ToggleGroupItem value="loop" class="h-8 px-3 text-xs">Loop</ToggleGroupItem>
        </ToggleGroup>
      </FormField>
      <div class="flex-1" />
      <Button size="sm" variant="outline" class="h-8" :disabled="!changed || !!error || busy" @click="apply">
        Apply
      </Button>
      <Button
        size="sm"
        variant="outline"
        class="h-8"
        :disabled="item.missing || changed || busy"
        :title="changed ? 'Apply the changes first' : 'On air, paused at its in point, ready to play'"
        @click="emit('cue')"
      >
        Show first frame
      </Button>
      <Button
        size="sm"
        class="h-8 min-w-20 gap-1.5"
        :variant="onAir && !onAirStale ? 'outline' : 'default'"
        :disabled="item.missing || changed || busy"
        :title="changed ? 'Apply the changes first' : 'Play it now (Enter)'"
        @click="emit('play')"
      >
        <Play class="size-3.5 fill-current" /> {{ onAir ? 'Play again' : 'Play' }}
      </Button>
    </div>
    <p v-if="error" class="text-xs text-destructive">{{ error }}</p>
    <p v-else-if="onAirStale" class="text-xs text-muted-foreground">
      It's on air with its old in and out points: play it again to use the new ones.
    </p>
  </section>
</template>
