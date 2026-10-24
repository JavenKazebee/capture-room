import { computed, ref, toValue, type MaybeRefOrGetter } from 'vue'

/**
 * A multi-selection by key: plain click selects one, Ctrl/⌘-click toggles,
 * Shift-click selects a range from the last one clicked.
 *
 * `order` is the keys in the order they're shown, for ranges and select-all;
 * either can also be given the keys at the call (e.g. a table's sorted rows).
 */
export function useSelection(order: MaybeRefOrGetter<string[]> = []) {
  const selected = ref(new Set<string>())
  /** The last key clicked: where Shift-click ranges start. */
  const focusedKey = ref<string | null>(null)
  const count = computed(() => selected.value.size)

  /** Plain click: select just this one. Toggle (Ctrl/Cmd-click or the checkbox): add or remove it. */
  function select(key: string, mode: 'only' | 'toggle' = 'only') {
    const next = new Set(mode === 'toggle' ? selected.value : [])
    if (mode === 'toggle' && next.has(key)) {
      next.delete(key)
      if (focusedKey.value === key) focusedKey.value = [...next].at(-1) ?? null
    } else {
      next.add(key)
      focusedKey.value = key
    }
    selected.value = next
  }

  /** Shift-click: select the shown range from the focused key to this one. */
  function selectRange(key: string, keys: string[] = toValue(order)) {
    const from = keys.indexOf(focusedKey.value ?? key)
    const to = keys.indexOf(key)
    if (from === -1 || to === -1) return select(key)
    const [a, b] = from < to ? [from, to] : [to, from]
    selected.value = new Set([...selected.value, ...keys.slice(a, b + 1)])
    focusedKey.value = key
  }

  function selectAll(keys: string[] = toValue(order)) {
    selected.value = new Set(keys)
    if (!focusedKey.value) focusedKey.value = keys[0] ?? null
  }

  function clearSelection() {
    selected.value = new Set()
    focusedKey.value = null
  }

  /** Unselect these keys, e.g. the items a bulk action succeeded on. */
  function deselect(keys: Iterable<string>) {
    const next = new Set(selected.value)
    for (const k of keys) next.delete(k)
    if (next.size === selected.value.size) return
    selected.value = next
    if (focusedKey.value && !next.has(focusedKey.value)) focusedKey.value = [...next].at(-1) ?? null
  }

  /** Keep only keys that still exist, after the data reloads. */
  function retain(existing: Iterable<string>) {
    if (!selected.value.size) return
    const keep = new Set(existing)
    deselect([...selected.value].filter((k) => !keep.has(k)))
  }

  return { selected, focusedKey, count, select, selectRange, selectAll, clearSelection, deselect, retain }
}

export type Selection = ReturnType<typeof useSelection>
