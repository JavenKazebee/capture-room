<script setup lang="ts" generic="T">
import { computed, ref, watch } from 'vue'
import { ArrowDown, ArrowUp, ChevronsUpDown } from '@lucide/vue'
import { usePreferences } from '@/composables/usePreferences'
import type { Selection } from '@/composables/useSelection'
import { cn } from '@/lib/utils'
import { Checkbox } from '@/components/ui/checkbox'

import type { Column } from './dataTable'
import { rowNavTarget } from './rowNav'

const props = defineProps<{
  tableId: string
  columns: Column<T>[]
  rows: T[]
  rowKey: (row: T) => string
  /** Text matched against every column's `value`. */
  filter?: string
  /** Group rows under a heading; rows keep their sorted order within a group. */
  groupBy?: ((row: T) => string) | null
  groupLabel?: (key: string) => string
  rowClass?: (row: T) => string | undefined
  emptyText?: string
  /**
   * Opt-in multiselect by `rowKey`: a checkbox column, Shift-click for a range,
   * Ctrl/⌘-click to toggle. A plain row click still emits `rowClick`.
   */
  selection?: Selection
}>()

const emit = defineEmits<{
  /** A click, or Enter on a focused row. */
  rowClick: [row: T, event: MouseEvent | KeyboardEvent]
  /** Focus moved to a row with the arrows, Home or End. */
  rowNavigate: [row: T]
}>()

const { columns: columnPrefs } = usePreferences()

const visibleColumns = computed(() => {
  const prefs = columnPrefs.value[props.tableId] ?? {}
  return props.columns.filter((c) => c.alwaysVisible || (prefs[c.id] ?? !c.hiddenByDefault))
})

const sort = ref<{ id: string; dir: 'asc' | 'desc' } | null>(null)

function toggleSort(c: Column<T>) {
  if (!c.value) return
  if (sort.value?.id !== c.id) sort.value = { id: c.id, dir: 'asc' }
  else if (sort.value.dir === 'asc') sort.value = { id: c.id, dir: 'desc' }
  else sort.value = null
}

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' })

const processed = computed(() => {
  let rows = props.rows
  const q = props.filter?.trim().toLowerCase()
  if (q) {
    rows = rows.filter((r) =>
      props.columns.some((c) => String(c.value?.(r) ?? '').toLowerCase().includes(q)),
    )
  }
  const s = sort.value
  const col = s && props.columns.find((c) => c.id === s.id)
  if (s && col?.value) {
    const get = col.value
    rows = [...rows].sort((a, b) => {
      const va = get(a)
      const vb = get(b)
      const cmp =
        va == null ? 1 : vb == null ? -1 : typeof va === 'number' && typeof vb === 'number' ? va - vb : collator.compare(String(va), String(vb))
      return s.dir === 'asc' ? cmp : -cmp
    })
  }
  return rows
})

const groups = computed(() => {
  if (!props.groupBy) return [{ key: '', rows: processed.value }]
  const map = new Map<string, T[]>()
  for (const r of processed.value) {
    const k = props.groupBy(r)
    if (!map.has(k)) map.set(k, [])
    map.get(k)!.push(r)
  }
  return [...map].map(([key, rows]) => ({ key, rows }))
})

/** Rows in the order they're shown: filtered, sorted, then grouped. */
const displayedRows = computed(() => groups.value.flatMap((g) => g.rows))
defineExpose({ displayedRows })

const colspan = computed(() => visibleColumns.value.length + (props.selection ? 1 : 0))

const displayedKeys = computed(() => displayedRows.value.map(props.rowKey))
// Rows a filter hides leave the selection, so a bulk action only touches rows in view.
watch(displayedKeys, (keys) => props.selection?.retain(keys))
const isSelected = (r: T) => !!props.selection?.selected.value.has(props.rowKey(r))

/** The header checkbox: over the rows shown, so a filter narrows what it selects. */
const allState = computed<boolean | 'indeterminate'>(() => {
  const sel = props.selection?.selected.value
  if (!sel?.size || !displayedKeys.value.length) return false
  const n = displayedKeys.value.filter((k) => sel.has(k)).length
  return n === 0 ? false : n === displayedKeys.value.length ? true : 'indeterminate'
})

function toggleAll() {
  const s = props.selection
  if (!s) return
  if (allState.value === true) s.deselect(displayedKeys.value)
  else s.selectAll([...new Set([...s.selected.value, ...displayedKeys.value])])
}

/** The checkbox cell: toggles, or Shift-click for a range. */
function onCheck(r: T, e: MouseEvent) {
  const s = props.selection!
  if (e.shiftKey) s.selectRange(props.rowKey(r), displayedKeys.value)
  else s.select(props.rowKey(r), 'toggle')
}

/** Clicks on a row's own buttons, links and switches are theirs, even with a modifier. */
const INTERACTIVE = 'button, a, input, select, textarea, label, [role="switch"], [role="checkbox"]'

function onRowClick(r: T, e: MouseEvent) {
  const s = props.selection
  const modified = e.shiftKey || e.metaKey || e.ctrlKey
  if (s && modified && !(e.target as HTMLElement).closest(INTERACTIVE)) {
    if (e.shiftKey) s.selectRange(props.rowKey(r), displayedKeys.value)
    else s.select(props.rowKey(r), 'toggle')
    return
  }
  emit('rowClick', r, e)
}

const root = ref<HTMLElement>()

/** The row Tab lands on (roving tabindex): the last one focused while shown, else the first. */
const focusKey = ref<string | null>(null)
const tabStop = computed(() =>
  focusKey.value != null && displayedKeys.value.includes(focusKey.value) ? focusKey.value : (displayedKeys.value[0] ?? null),
)

function onRowKeyDown(r: T, e: KeyboardEvent) {
  // Keys on a row's own buttons, links and switches are theirs.
  if (e.target !== e.currentTarget || e.ctrlKey || e.metaKey || e.altKey) return
  if (e.key === 'Enter') {
    e.preventDefault()
    emit('rowClick', r, e)
    return
  }
  const keys = displayedKeys.value
  const i = rowNavTarget(e.key, keys.indexOf(props.rowKey(r)), keys.length)
  if (i == null) return
  e.preventDefault()
  focusKey.value = keys[i]!
  root.value?.querySelector<HTMLElement>(`[data-row-key="${CSS.escape(keys[i]!)}"]`)?.focus()
  emit('rowNavigate', displayedRows.value[i]!)
}

const ariaSort = (c: Column<T>) =>
  !c.value ? undefined : sort.value?.id !== c.id ? 'none' : sort.value.dir === 'asc' ? 'ascending' : 'descending'

/** Shift-click would otherwise select the text between clicks. */
function onRowMouseDown(e: MouseEvent) {
  if (props.selection && e.shiftKey) e.preventDefault()
}
</script>

<template>
  <div ref="root" class="rounded-lg border border-border bg-card overflow-x-auto">
    <table class="w-full text-xs">
      <thead class="bg-muted/40 text-muted-foreground">
        <tr class="border-b border-border">
          <th v-if="selection" class="h-8 w-px pl-3 pr-0 cursor-pointer" @click="toggleAll">
            <Checkbox
              :model-value="allState"
              :disabled="!displayedRows.length"
              aria-label="Select all rows shown"
              class="align-middle"
            />
          </th>
          <th
            v-for="c in visibleColumns"
            :key="c.id"
            class="h-8 px-3 font-medium whitespace-nowrap select-none"
            :class="[c.align === 'right' ? 'text-right' : 'text-left', c.headerClass]"
            :aria-sort="ariaSort(c)"
          >
            <button
              v-if="c.value"
              class="inline-flex items-center gap-1 hover:text-foreground"
              :class="sort?.id === c.id && 'text-foreground'"
              @click="toggleSort(c)"
            >
              {{ c.label }}
              <ArrowUp v-if="sort?.id === c.id && sort.dir === 'asc'" class="size-3 text-primary" />
              <ArrowDown v-else-if="sort?.id === c.id" class="size-3 text-primary" />
              <ChevronsUpDown v-else class="size-3 opacity-40" />
            </button>
            <span v-else>{{ c.label }}</span>
          </th>
        </tr>
      </thead>
      <tbody v-for="g in groups" :key="g.key">
        <tr v-if="groupBy" class="bg-muted/20 border-b border-border">
          <td :colspan="colspan" class="px-3 py-1.5 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            {{ groupLabel ? groupLabel(g.key) : g.key }}
            <span class="num font-normal ml-1">{{ g.rows.length }}</span>
          </td>
        </tr>
        <tr
          v-for="r in g.rows"
          :key="rowKey(r)"
          :data-row-key="rowKey(r)"
          :data-state="isSelected(r) ? 'selected' : undefined"
          :tabindex="rowKey(r) === tabStop ? 0 : -1"
          :class="
            cn(
              'border-b border-border/60 last:border-b-0 hover:bg-accent/40 data-[state=selected]:bg-primary/5 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring',
              rowClass?.(r),
            )
          "
          @click="onRowClick(r, $event)"
          @mousedown="onRowMouseDown"
          @keydown="onRowKeyDown(r, $event)"
          @focusin="focusKey = rowKey(r)"
        >
          <td v-if="selection" class="w-px pl-3 pr-0 py-1.5 align-middle cursor-pointer" @click.stop="onCheck(r, $event)">
            <Checkbox :model-value="isSelected(r)" aria-label="Select row" class="align-middle" />
          </td>
          <td
            v-for="c in visibleColumns"
            :key="c.id"
            class="px-3 py-1.5 align-middle"
            :class="[c.align === 'right' && 'text-right', c.class]"
          >
            <slot :name="`cell-${c.id}`" :row="r">{{ c.value?.(r) ?? '—' }}</slot>
          </td>
        </tr>
      </tbody>
      <tbody v-if="processed.length === 0">
        <tr>
          <td :colspan="colspan" class="px-3 py-8 text-center text-muted-foreground">
            <slot name="empty">{{ filter ? 'No rows match the filter.' : (emptyText ?? 'Nothing here yet.') }}</slot>
          </td>
        </tr>
      </tbody>
    </table>
  </div>
</template>
