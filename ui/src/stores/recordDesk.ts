import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { useStorage } from '@vueuse/core'
import { toast } from 'vue-sonner'
import { formatTimeLeft } from '@/lib/format'
import { notifyError } from '@/lib/notify'
import { LOW_TIME_SECS, useCapacityStore } from '@/stores/capacity'
import { useEventsStore } from '@/stores/events'
import { useNodesStore } from '@/stores/nodes'
import { blankLeg, presetLegs, usePresetsStore } from '@/stores/presets'
import { useRecordingsStore } from '@/stores/recordings'
import { useSourcesStore, type Source } from '@/stores/sources'

export type StateFilter = 'all' | 'live' | 'idle'

/** Something to tell the operator about recordings they're starting. */
interface Warning {
  nodeId: string
  text: string
}

/**
 * State of the Record workspace: which feeds are selected, which one the
 * inspector shows, each feed's chosen preset, and the multiview filters.
 */
export const useRecordDeskStore = defineStore('recordDesk', () => {
  const sources = useSourcesStore()
  const recordings = useRecordingsStore()
  const presets = usePresetsStore()
  const capacity = useCapacityStore()
  const nodes = useNodesStore()

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

  /** Set the preset for several feeds at once (used for their next recording). */
  function setPresetMany(keys: string[], id: string) {
    const next = { ...presetFor.value }
    for (const k of keys) next[k] = id
    presetFor.value = next
  }

  const MIXED = '__mixed'

  /** The selection's shared preset id, or `MIXED` when they differ. */
  const selectedPresetId = computed(() => {
    const ids = new Set(selectedSources.value.map((s) => presetIdOf(s.key)))
    return ids.size === 1 ? [...ids][0]! : MIXED
  })

  /** Preset options for the selection's picker, with a "Mixed" entry when they differ. */
  const selectionPresetOptions = computed(() =>
    selectedPresetId.value === MIXED
      ? [{ value: MIXED, label: 'Mixed presets' }, ...presetOptions.value]
      : presetOptions.value,
  )

  function setSelectionPreset(id: string) {
    if (id !== MIXED) setPresetMany([...selected.value], id)
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

  /**
   * What to warn about before starting `list`: a node pushed past (or close
   * to) its benchmarked capacity, or a volume that would fill within the
   * hour. Checked before starting, so the new recordings aren't counted
   * twice. Never blocks recording: a failed check just warns about nothing.
   */
  async function capacityWarnings(list: Source[]): Promise<Warning[]> {
    const groups = new Map<string, { nodeId: string; presetId: string; sourceIds: string[] }>()
    for (const s of list) {
      const presetId = presetIdOf(s.key)
      const key = `${s.node_id}\n${presetId}`
      const group = groups.get(key) ?? { nodeId: s.node_id, presetId, sourceIds: [] }
      group.sourceIds.push(s.id)
      groups.set(key, group)
    }
    const checks = await Promise.all(
      [...groups.values()].map(async (g) => {
        const preset = presets.presets.find((p) => p.id === g.presetId)
        const outputs = preset ? presetLegs(preset) : [blankLeg()]
        return { ...g, check: await capacity.check(g.nodeId, outputs, g.sourceIds).catch(() => null) }
      }),
    )

    const warnings: Warning[] = []
    const perNode = new Map<string, { before: number; added: number }>()
    const lowest = new Map<string, { nodeId: string; mount: string; secs: number }>()
    for (const { nodeId, check } of checks) {
      if (!check) continue
      // Groups for one node each start from the same load, so add their increases.
      const n = perNode.get(nodeId) ?? { before: check.load_before, added: 0 }
      n.added += check.load_after - check.load_before
      perNode.set(nodeId, n)
      for (const v of check.volumes) {
        const key = `${nodeId}\n${v.mount_point}`
        if (v.seconds_left != null && v.seconds_left < (lowest.get(key)?.secs ?? Infinity)) {
          lowest.set(key, { nodeId, mount: v.mount_point, secs: v.seconds_left })
        }
      }
    }
    for (const [nodeId, { before, added }] of perNode) {
      const pct = Math.round((before + added) * 100)
      const name = nodes.nameOf(nodeId)
      if (pct > 100) warnings.push({ nodeId, text: `${name} is over its benchmarked capacity (${pct}%): expect dropped frames.` })
      else if (pct > 80) warnings.push({ nodeId, text: `${name} is near its benchmarked capacity (${pct}%).` })
    }
    for (const { nodeId, mount, secs } of lowest.values()) {
      if (secs < LOW_TIME_SECS) {
        warnings.push({ nodeId, text: `${mount} on ${nodes.nameOf(nodeId)} fills in about ${formatTimeLeft(secs)} at this rate.` })
      }
    }
    return warnings
  }

  function showWarnings(warnings: Warning[]) {
    const log = useEventsStore().log
    for (const w of warnings) {
      toast.warning(w.text)
      log('warn', w.text, { node_id: w.nodeId })
    }
  }

  /** Start or stop one source; failures go to a toast and the event log. */
  async function toggle(s: Source) {
    if (busy.value.has(s.key)) return
    const live = isLive(s)
    busy.value = new Set(busy.value).add(s.key)
    try {
      if (live) {
        await stop(s)
      } else {
        const warnings = await capacityWarnings([s])
        await start(s)
        showWarnings(warnings)
      }
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
    const warnings = action === 'start' ? await capacityWarnings(targets) : []
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
    if (ok) showWarnings(warnings)
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
    setPresetMany,
    presetOptions,
    selectedPresetId,
    selectionPresetOptions,
    setSelectionPreset,
    busy,
    isLive,
    toggle,
    bulk,
  }
})
