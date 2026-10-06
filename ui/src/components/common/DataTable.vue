<script setup lang="ts" generic="T">
import { computed, ref } from 'vue'
import { ArrowDown, ArrowUp, ChevronsUpDown } from '@lucide/vue'
import { usePreferences } from '@/composables/usePreferences'
import { cn } from '@/lib/utils'

import type { Column } from './dataTable'

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
}>()

const emit = defineEmits<{ rowClick: [row: T, event: MouseEvent] }>()

const { columns: columnPrefs } = usePreferences()

const visibleColumns = computed(() => {
  const prefs = columnPrefs.value[props.tableId] ?? {}
  return props.columns.filter((c) => c.alwaysVisible || (prefs[c.id] ?? !c.hiddenByDefault))
})

// ── Sorting ───────────────────────────────────────────────────────────────────

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
</script>

<template>
  <div class="rounded-lg border border-border bg-card overflow-x-auto">
    <table class="w-full text-xs">
      <thead class="bg-muted/40 text-muted-foreground">
        <tr class="border-b border-border">
          <th
            v-for="c in visibleColumns"
            :key="c.id"
            class="h-8 px-3 font-medium whitespace-nowrap select-none"
            :class="[c.align === 'right' ? 'text-right' : 'text-left', c.headerClass]"
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
          <td :colspan="visibleColumns.length" class="px-3 py-1.5 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            {{ groupLabel ? groupLabel(g.key) : g.key }}
            <span class="num font-normal ml-1">{{ g.rows.length }}</span>
          </td>
        </tr>
        <tr
          v-for="r in g.rows"
          :key="rowKey(r)"
          :data-row-key="rowKey(r)"
          :class="cn('border-b border-border/60 last:border-b-0 hover:bg-accent/40', rowClass?.(r))"
          @click="emit('rowClick', r, $event)"
        >
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
          <td :colspan="visibleColumns.length" class="px-3 py-8 text-center text-muted-foreground">
            <slot name="empty">{{ filter ? 'No rows match the filter.' : (emptyText ?? 'Nothing here yet.') }}</slot>
          </td>
        </tr>
      </tbody>
    </table>
  </div>
</template>
