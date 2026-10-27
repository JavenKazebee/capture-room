<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Circle, MonitorPlay, Trash2 } from '@lucide/vue'
import { errorMessage, sourceKey } from '@/composables/useApi'
import { pad } from '@/lib/format'
import { addToTime, monthlyFrom, secondsBetween, WEEKDAY_NAMES, weekOfMonthLabel } from '@/lib/schedules'
import { useNodesStore } from '@/stores/nodes'
import { usePlayoutStore } from '@/stores/playout'
import { usePresetsStore } from '@/stores/presets'
import { useSchedulesStore, WEEKDAYS } from '@/stores/schedules'
import { useSourcesStore } from '@/stores/sources'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import EditSheet from '@/components/common/EditSheet.vue'
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'
import type { ScheduleAction } from '@/types/generated/ScheduleAction'
import type { ScheduleDto } from '@/types/generated/ScheduleDto'
import type { ScheduleInput } from '@/types/generated/ScheduleInput'
import type { ScheduleKind } from '@/types/generated/ScheduleKind'

const props = defineProps<{ schedule: ScheduleDto | null }>()
const emit = defineEmits<{ close: [] }>()

const store = useSchedulesStore()
const nodes = useNodesStore()
const sources = useSourcesStore()
const presets = usePresetsStore()
const playout = usePlayoutStore()

/** A form row: sources by key, a channel by key. */
type RecordRow = { kind: 'record'; targets: string[]; preset_id: string }
type PlayoutRow = { kind: 'playout'; channel: string; item_id: string; stop_at_end: boolean }
type Row = RecordRow | PlayoutRow

/** The start item meaning "where it's cued" (a Select can't hold ''). */
const CUED = 'cued'

const today = new Date()
const todayDate = `${today.getFullYear()}-${pad(today.getMonth() + 1)}-${pad(today.getDate())}`
// Monday is bit 0, as on the server.
const todayBit = (today.getDay() + 6) % 7

const s = props.schedule
const name = ref(s?.name ?? '')
const enabled = ref(s?.enabled ?? true)
const kind = ref<ScheduleKind>(s?.kind ?? 'weekly')
const date = ref(s?.date ?? todayDate)
const days = ref<string[]>(
  s ? WEEKDAYS.map((_, i) => String(i)).filter((i) => s.weekdays & (1 << Number(i))) : [String(todayBit)],
)
// Monthly: a day of the month, or the nth (-1: last) of a weekday. Both
// follow the start date as it changes, as in a calendar.
const seed = monthlyFrom(date.value)
const monthBy = ref<'day' | 'weekday'>(s?.month_week != null ? 'weekday' : 'day')
const monthDay = ref<number | null>(s?.month_day ?? seed.day)
const monthWeek = ref(String(s?.month_week ?? seed.week))
const monthWeekday = ref(String(s?.month_week != null ? Math.max(0, WEEKDAYS.findIndex((_, i) => s.weekdays & (1 << i))) : seed.weekday))
watch(date, (d) => {
  if (!d) return
  const m = monthlyFrom(d)
  monthDay.value = m.day
  monthWeek.value = String(m.week)
  monthWeekday.value = String(m.weekday)
})
const weekOptions = [1, 2, 3, 4, -1].map((w) => ({ value: String(w), label: weekOfMonthLabel(w) }))
const weekdayOptions = WEEKDAY_NAMES.map((label, i) => ({ value: String(i), label }))

const startTime = ref(s?.start_time ?? '09:00')
const hasEnd = ref(s ? s.duration_s != null : true)
const endTime = ref(s?.duration_s ? addToTime(s.start_time, s.duration_s) : '10:00')

function toRow(a: ScheduleAction): Row {
  return a.kind === 'record'
    ? { kind: 'record', targets: a.targets.map((t) => sourceKey(t.node_id, t.source_id)), preset_id: a.preset_id }
    : { kind: 'playout', channel: sourceKey(a.node_id, a.channel_id), item_id: a.item_id ?? CUED, stop_at_end: a.stop_at_end }
}
const rows = ref<Row[]>(s ? s.actions.map(toRow) : [])

const recordable = computed(() => sources.sources.filter((x) => x.source_type !== 'channel'))
const byNode = computed(() => {
  const groups = new Map<string, typeof recordable.value>()
  for (const src of recordable.value) groups.set(src.node_id, [...(groups.get(src.node_id) ?? []), src])
  return [...groups.entries()]
})
const multiNode = computed(() => nodes.nodes.length > 1)
const channelOptions = computed(() =>
  playout.channels.map((c) => ({
    value: c.key,
    label: multiNode.value ? `${c.display_name} — ${nodes.nameOf(c.node_id)}` : c.display_name,
  })),
)

function addRecord() {
  rows.value.push({ kind: 'record', targets: [], preset_id: presets.options[0]?.value ?? 'default' })
}
function addPlayout() {
  rows.value.push({ kind: 'playout', channel: channelOptions.value[0]?.value ?? '', item_id: CUED, stop_at_end: true })
}

function toggleTarget(row: RecordRow, key: string, on: boolean) {
  row.targets = on ? [...row.targets, key] : row.targets.filter((k) => k !== key)
}

function splitKey(key: string) {
  const i = key.indexOf('/')
  return { node_id: key.slice(0, i), id: key.slice(i + 1) }
}

// Each channel's playlist, for its start item.
watch(
  () => rows.value.filter((r): r is PlayoutRow => r.kind === 'playout').map((r) => r.channel),
  (keys) => {
    for (const key of keys) {
      if (!key || playout.playlists.has(key)) continue
      const { node_id, id } = splitKey(key)
      playout.loadPlaylist(node_id, id).catch(() => {})
    }
  },
  { immediate: true },
)

function itemOptions(channel: string) {
  return [
    { value: CUED, label: 'Where it’s cued' },
    ...(playout.playlists.get(channel)?.items ?? []).map((i) => ({ value: i.id, label: i.name })),
  ]
}

function toAction(r: Row): ScheduleAction {
  if (r.kind === 'record') {
    return {
      kind: 'record',
      targets: r.targets.map((k) => {
        const { node_id, id } = splitKey(k)
        return { node_id, source_id: id }
      }),
      preset_id: r.preset_id,
    }
  }
  const { node_id, id } = splitKey(r.channel)
  return { kind: 'playout', node_id, channel_id: id, item_id: r.item_id === CUED ? null : r.item_id, stop_at_end: r.stop_at_end }
}

const formError = ref<string | null>(null)
const saving = ref(false)

async function save() {
  formError.value = null
  const monthly = kind.value === 'monthly'
  const byWeekday = monthBy.value === 'weekday'
  const input: ScheduleInput = {
    name: name.value,
    enabled: enabled.value,
    kind: kind.value,
    // A monthly schedule's first run is on or after its date.
    date: kind.value === 'weekly' ? null : date.value,
    weekdays:
      kind.value === 'weekly'
        ? days.value.reduce((bits, d) => bits | (1 << Number(d)), 0)
        : monthly && byWeekday
          ? 1 << Number(monthWeekday.value)
          : 0,
    month_day: monthly && !byWeekday ? monthDay.value : null,
    month_week: monthly && byWeekday ? Number(monthWeek.value) : null,
    start_time: startTime.value,
    duration_s: hasEnd.value ? secondsBetween(startTime.value, endTime.value) : null,
    actions: rows.value.map(toAction),
  }
  saving.value = true
  try {
    if (props.schedule) await store.update(props.schedule.id, input)
    else await store.create(input)
    emit('close')
  } catch (e) {
    formError.value = errorMessage(e, 'Couldn’t save the schedule')
  } finally {
    saving.value = false
  }
}

const confirmDelete = ref(false)
async function remove() {
  if (!props.schedule) return
  try {
    await store.remove(props.schedule.id)
    emit('close')
  } catch (e) {
    formError.value = errorMessage(e, 'Couldn’t delete the schedule')
  }
}
</script>

<template>
  <EditSheet
    :title="schedule ? 'Edit schedule' : 'New schedule'"
    description="Starts recordings and playout at a set time, in this controller’s local time, and stops them at the end."
    :error="formError"
    :saving="saving"
    wide
    @close="emit('close')"
    @save="save"
  >
    <div class="grid grid-cols-2 gap-3">
      <FormField label="Name" class="col-span-2">
        <Input v-model="name" placeholder="Sunday service" />
      </FormField>

      <FormField label="Repeats" class="col-span-2">
        <ToggleGroup
          :model-value="kind"
          type="single"
          variant="segmented"
          class="w-fit"
          @update:model-value="(v) => v && (kind = v as ScheduleKind)"
        >
          <ToggleGroupItem value="weekly" class="px-3">Weekly</ToggleGroupItem>
          <ToggleGroupItem value="monthly" class="px-3">Monthly</ToggleGroupItem>
          <ToggleGroupItem value="once" class="px-3">Once</ToggleGroupItem>
        </ToggleGroup>
      </FormField>

      <FormField v-if="kind === 'weekly'" label="On" class="col-span-2">
        <ToggleGroup v-model="days" type="multiple" variant="segmented" class="w-fit">
          <ToggleGroupItem v-for="(d, i) in WEEKDAYS" :key="d" :value="String(i)" class="px-2.5">{{ d }}</ToggleGroupItem>
        </ToggleGroup>
      </FormField>
      <FormField v-else :label="kind === 'monthly' ? 'Starting' : 'Date'" class="col-span-2">
        <Input v-model="date" type="date" class="w-44" />
      </FormField>
      <FormField v-if="kind === 'monthly'" label="On" class="col-span-2">
        <div class="space-y-2">
          <label class="flex items-center gap-2 text-sm">
            <input v-model="monthBy" type="radio" value="day" class="accent-primary" />
            On day
            <Input v-model.number="monthDay" type="number" min="1" max="31" class="num w-16" @focus="monthBy = 'day'" />
            <span v-if="(monthDay ?? 0) > 28" class="text-xs text-muted-foreground">Months without it are skipped</span>
          </label>
          <label class="flex items-center gap-2 text-sm">
            <input v-model="monthBy" type="radio" value="weekday" class="accent-primary" />
            On the
            <OptionSelect v-model="monthWeek" :options="weekOptions" class="w-20" @update:model-value="monthBy = 'weekday'" />
            <OptionSelect v-model="monthWeekday" :options="weekdayOptions" class="w-32" @update:model-value="monthBy = 'weekday'" />
          </label>
        </div>
      </FormField>

      <FormField label="Start">
        <Input v-model="startTime" type="time" />
      </FormField>
      <FormField>
        <template #label>
          <span class="flex items-center gap-2">
            <Switch v-model="hasEnd" class="scale-75 -ml-1" /> {{ hasEnd ? 'End' : 'No end: nothing is stopped' }}
          </span>
        </template>
        <Input v-model="endTime" type="time" :disabled="!hasEnd" />
      </FormField>

      <div class="col-span-2 flex items-center gap-2 pt-2">
        <h3 class="text-xs font-semibold uppercase tracking-wider text-muted-foreground">What it does</h3>
        <div class="flex-1" />
        <Button type="button" variant="outline" size="sm" @click="addRecord"><Circle /> Add recording</Button>
        <Button type="button" variant="outline" size="sm" :disabled="!channelOptions.length" @click="addPlayout">
          <MonitorPlay /> Add playout
        </Button>
      </div>
      <p v-if="!rows.length" class="col-span-2 text-xs text-muted-foreground">
        Add the sources to record, or a channel to play.
      </p>

      <section
        v-for="(row, idx) in rows"
        :key="idx"
        class="col-span-2 rounded-md border border-border p-3 space-y-3"
      >
        <div class="flex items-center gap-2 text-sm font-medium">
          <Circle v-if="row.kind === 'record'" class="size-3.5 text-tally" />
          <MonitorPlay v-else class="size-3.5 text-info" />
          {{ row.kind === 'record' ? 'Record' : 'Play a channel' }}
          <div class="flex-1" />
          <button type="button" class="text-muted-foreground hover:text-destructive" aria-label="Remove" @click="rows.splice(idx, 1)">
            <Trash2 class="size-3.5" />
          </button>
        </div>

        <template v-if="row.kind === 'record'">
          <FormField label="Preset">
            <OptionSelect v-model="row.preset_id" :options="presets.options" />
          </FormField>
          <div class="space-y-2">
            <span class="text-xs text-muted-foreground">Sources, all starting on the same frame</span>
            <p v-if="!recordable.length" class="text-xs text-muted-foreground">No sources yet.</p>
            <div v-for="[nodeId, list] in byNode" :key="nodeId" class="space-y-1">
              <p v-if="multiNode" class="text-[11px] font-medium text-muted-foreground">{{ nodes.labelOf(nodeId) }}</p>
              <div class="grid grid-cols-2 gap-x-3 gap-y-1">
                <label v-for="src in list" :key="src.key" class="flex items-center gap-2 text-sm min-w-0">
                  <input
                    type="checkbox"
                    class="accent-primary"
                    :checked="row.targets.includes(src.key)"
                    @change="toggleTarget(row, src.key, ($event.target as HTMLInputElement).checked)"
                  />
                  <span class="truncate">{{ src.display_name }}</span>
                </label>
              </div>
            </div>
          </div>
        </template>

        <div v-else class="grid grid-cols-2 gap-3">
          <FormField label="Channel">
            <OptionSelect v-model="row.channel" :options="channelOptions" />
          </FormField>
          <FormField label="Start from">
            <OptionSelect v-model="row.item_id" :options="itemOptions(row.channel)" />
          </FormField>
          <label class="col-span-2 flex items-center gap-2 text-sm">
            <Switch v-model="row.stop_at_end" :disabled="!hasEnd" />
            Stop the channel at the end
          </label>
        </div>
      </section>

      <label class="col-span-2 flex items-center gap-2 text-sm pt-2">
        <Switch v-model="enabled" /> Enabled
      </label>
    </div>

    <template v-if="schedule" #footer-start>
      <Button type="button" variant="ghost" class="text-destructive" @click="confirmDelete = true">Delete</Button>
    </template>
  </EditSheet>

  <ConfirmDialog
    v-model:open="confirmDelete"
    :title="`Delete “${schedule?.name}”?`"
    description="If it’s running, what it started is stopped first. Its history goes too."
    @confirm="remove"
  />
</template>
