import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { useStorage } from '@vueuse/core'
import { toast } from 'vue-sonner'
import { notifyError } from '@/lib/notify'
import { usePresetsStore } from '@/stores/presets'
import { useRecordingsStore } from '@/stores/recordings'
import { useSourcesStore, type Source } from '@/stores/sources'

export type StateFilter = 'all' | 'live' | 'idle'

/**
 * State of the Record workspace: which feeds are selected, which one the
 * inspector shows, each feed's chosen preset, and the multiview filters.
 */
export const useRecordDeskStore = defineStore('recordDesk', () => {
  const sources = useSourcesStore()
  const recordings = useRecordingsStore()
  const presets = usePresetsStore()

  // ── Filters ────────────────────────────────────────────────────────────────

  const query = ref('')
  const nodeFilter = ref<string>('all')
  const typeFilter = ref<string>('all')
  const stateFilter = ref<StateFilter>('all')

  const isLive = (s: Source) => !!recordings.activeForSource(s.node_id, s.id)

  const visible = computed(() => {
    const q = query.value.trim().toLowerCase()
    return sources.sources.filter(
      (s) =>
        (nodeFilter.value === 'all' || s.node_id === nodeFilter.value) &&
        (typeFilter.value === 'all' || s.source_type === typeFilter.value) &&
        (stateFilter.value === 'all' || (stateFilter.value === 'live') === isLive(s)) &&
        (!q || s.display_name.toLowerCase().includes(q) || s.id.toLowerCase().includes(q)),
    )
  })

  const filtered = computed(
    () => !!query.value.trim() || nodeFilter.value !== 'all' || typeFilter.value !== 'all' || stateFilter.value !== 'all',
  )

  function clearFilters() {
    query.value = ''
    nodeFilter.value = typeFilter.value = 'all'
    stateFilter.value = 'all'
  }

  // ── Selection ──────────────────────────────────────────────────────────────

  /** Selected source keys. */
  const selected = ref(new Set<string>())
  /** The source the inspector shows: the last one clicked. */
  const focusedKey = ref<string | null>(null)

  const selectedSources = computed(() => sources.sources.filter((s) => selected.value.has(s.key)))
  const focused = computed(() => sources.sources.find((s) => s.key === focusedKey.value) ?? null)

  /** Plain click: select just this one. Toggle (Ctrl/⌘ or the checkbox): add or remove it. */
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

  /** Shift-click: select the visible range from the focused tile to this one. */
  function selectRange(key: string) {
    const keys = visible.value.map((s) => s.key)
    const from = keys.indexOf(focusedKey.value ?? key)
    const to = keys.indexOf(key)
    if (from === -1 || to === -1) return select(key)
    const [a, b] = from < to ? [from, to] : [to, from]
    selected.value = new Set([...selected.value, ...keys.slice(a, b + 1)])
    focusedKey.value = key
  }

  function selectAll() {
    selected.value = new Set(visible.value.map((s) => s.key))
    if (!focusedKey.value) focusedKey.value = visible.value[0]?.key ?? null
  }

  function clearSelection() {
    selected.value = new Set()
    focusedKey.value = null
  }

  // ── Presets per source ─────────────────────────────────────────────────────

  /** Preset id per source key; `default` is the built-in H.264/MOV output. */
  const presetFor = useStorage<Record<string, string>>('cr.record.presets', {})

  function presetIdOf(key: string) {
    const id = presetFor.value[key] ?? 'default'
    // A deleted preset falls back to the default visibly.
    return id === 'default' || presets.presets.some((p) => p.id === id) ? id : 'default'
  }

  function setPreset(key: string, id: string) {
    presetFor.value = { ...presetFor.value, [key]: id }
  }

  const presetOptions = computed(() => [
    { value: 'default', label: 'H.264 (default)' },
    ...presets.presets.map((p) => ({ value: p.id, label: p.name })),
  ])

  // ── Start / stop ───────────────────────────────────────────────────────────

  const busy = ref(new Set<string>())

  async function start(s: Source) {
    const preset = presets.presets.find((p) => p.id === presetIdOf(s.key)) ?? null
    await recordings.start(s.node_id, s.id, preset)
  }

  async function stop(s: Source) {
    const session = recordings.activeForSource(s.node_id, s.id)
    if (session) await recordings.stop(s.node_id, session.id)
  }

  /** Start or stop one source; failures go to a toast and the event log. */
  async function toggle(s: Source) {
    if (busy.value.has(s.key)) return
    const live = isLive(s)
    busy.value = new Set(busy.value).add(s.key)
    try {
      await (live ? stop(s) : start(s))
    } catch (e) {
      notifyError(`${live ? 'Stop' : 'Record'} failed: ${s.display_name}`, e, s.node_id)
    } finally {
      const next = new Set(busy.value)
      next.delete(s.key)
      busy.value = next
    }
  }

  /** Start (or stop) several at once; reports one summary toast plus a log entry per failure. */
  async function bulk(action: 'start' | 'stop', list: Source[]) {
    const targets = list.filter((s) => (action === 'start' ? !isLive(s) : isLive(s)))
    if (!targets.length) return
    const results = await Promise.allSettled(targets.map((s) => (action === 'start' ? start(s) : stop(s))))
    let failed = 0
    results.forEach((r, i) => {
      if (r.status === 'rejected') {
        failed++
        const s = targets[i]!
        notifyError(`${action === 'start' ? 'Record' : 'Stop'} failed: ${s.display_name}`, r.reason, s.node_id)
      }
    })
    const ok = targets.length - failed
    if (ok) toast.success(`${action === 'start' ? 'Recording' : 'Stopped'} ${ok} feed${ok > 1 ? 's' : ''}`)
  }

  return {
    query,
    nodeFilter,
    typeFilter,
    stateFilter,
    visible,
    filtered,
    clearFilters,
    selected,
    focusedKey,
    selectedSources,
    focused,
    select,
    selectRange,
    selectAll,
    clearSelection,
    presetIdOf,
    setPreset,
    presetOptions,
    busy,
    isLive,
    toggle,
    bulk,
  }
})
