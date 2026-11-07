import { computed, ref, watch } from 'vue'
import { useUnsavedGuard } from '@/composables/useUnsavedGuard'
import { advancedProblems } from '@/lib/advanced'
import { clashingLegs, legProblems } from '@/lib/codecs'
import { blankLeg, usePresetsStore } from '@/stores/presets'
import type { PresetDto } from '@/types/generated/PresetDto'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'
import type { SlateSettings } from '@/types/generated/SlateSettings'
import { slateProblem } from '@/lib/slate'

/**
 * The preset being edited: a copy of a saved one or a new one, with what's blocking Save. Switching away from unsaved edits, or leaving the page,
 * goes through `guarded` and its confirm.
 */
export function usePresetDraft() {
  const store = usePresetsStore()

  /** A preset id, or `new`. */
  const selectedId = ref<string | null>(null)
  const name = ref('')
  const legs = ref<PresetOutputInput[]>([])
  const slate = ref<SlateSettings | null>(null)
  /** Stable keys for the output cards, moved and spliced alongside `legs`. */
  const legKeys = ref<number[]>([])
  let nextKey = 0
  const original = ref('')
  const serverError = ref<string | null>(null)
  /** Record feeds that asked for this draft ("New preset…"); it's applied to them once saved. */
  const pendingFor = ref<string[]>([])

  const selected = computed(() => store.presets.find((p) => p.id === selectedId.value) ?? null)
  const snapshot = () => JSON.stringify({ name: name.value, legs: legs.value, slate: slate.value })
  const dirty = computed(() => selectedId.value !== null && snapshot() !== original.value)

  const { confirmOpen, guarded, discard } = useUnsavedGuard(dirty)

  function load(id: string, draftName: string, draftLegs: PresetOutputInput[], draftSlate: SlateSettings | null = null) {
    // Leaving the draft for another preset drops the request.
    if (id !== 'new') pendingFor.value = []
    selectedId.value = id
    name.value = draftName
    legs.value = draftLegs.map((l) => ({ ...l }))
    legKeys.value = draftLegs.map(() => nextKey++)
    slate.value = draftSlate ? { ...draftSlate } : null
    original.value = id === 'new' ? '' : snapshot()
    serverError.value = null
  }

  function loadPreset(p: PresetDto) {
    load(p.id, p.name, p.outputs.length ? p.outputs : [blankLeg()], p.slate)
  }

  function select(id: string) {
    if (id === selectedId.value) return
    guarded(() => {
      const p = store.presets.find((p) => p.id === id)
      if (p) loadPreset(p)
    })
  }

  function duplicate() {
    const copyName = `${name.value || 'Preset'} copy`
    const copyLegs = legs.value
    const copySlate = slate.value
    guarded(() => load('new', copyName, copyLegs, copySlate))
    name.value = copyName
  }

  function revert() {
    if (selected.value) loadPreset(selected.value)
    else if (selectedId.value === 'new') load('new', '', [blankLeg()])
  }

  /** A new output starts as a copy of the first, its files told apart by {output}. */
  function addOutput() {
    const n = legs.value.length + 1
    const first: PresetOutputInput = legs.value[0] ? JSON.parse(JSON.stringify(legs.value[0])) : blankLeg()
    legs.value.push({ ...first, name: `Output ${n}`, preview: false, path_template: withOutputToken(first.path_template) })
    legKeys.value.push(nextKey++)
  }

  function duplicateOutput(i: number) {
    const leg = legs.value[i]!
    legs.value.splice(i + 1, 0, { ...leg, name: `${leg.name} copy`, preview: false })
    legKeys.value.splice(i + 1, 0, nextKey++)
  }

  /** Only one output is the preview: marking one unmarks the others. */
  function setPreview(i: number) {
    legs.value.forEach((leg, j) => {
      if (j !== i && leg.preview) legs.value[j] = { ...leg, preview: false }
    })
  }

  function removeOutput(i: number) {
    legs.value.splice(i, 1)
    legKeys.value.splice(i, 1)
  }

  function moveOutput(i: number, dir: -1 | 1) {
    for (const list of [legs.value, legKeys.value] as unknown[][]) {
      ;[list[i], list[i + dir]] = [list[i + dir], list[i]]
    }
  }

  const clashes = computed(() => clashingLegs(legs.value))
  /** What's blocking Save, in words: the name, then each output with a problem. */
  const problems = computed(() => {
    const list: string[] = []
    if (!name.value.trim()) list.push('Name the preset')
    legs.value.forEach((l, i) => {
      if (clashes.value.has(i) || Object.keys(legProblems(l)).length || Object.keys(advancedProblems(l)).length) {
        list.push(`Fix ${l.name || `output ${i + 1}`}`)
      }
    })
    if (slateProblem(slate.value)) list.push('Fix the slate')
    return list
  })

  // A preset updated elsewhere refreshes the editor unless there are local edits.
  watch(selected, (p) => p && !dirty.value && p.id === selectedId.value && loadPreset(p))

  return {
    selectedId,
    name,
    legs,
    slate,
    legKeys,
    serverError,
    pendingFor,
    selected,
    dirty,
    confirmOpen,
    guarded,
    discard,
    load,
    loadPreset,
    select,
    duplicate,
    revert,
    addOutput,
    duplicateOutput,
    setPreview,
    removeOutput,
    moveOutput,
    clashes,
    problems,
  }
}

/** `template` with `{output}` at the start of its file name, so a second output doesn't write the first one's file. */
export function withOutputToken(template: string) {
  if (template.includes('{output}')) return template
  const cut = Math.max(template.lastIndexOf('/'), template.lastIndexOf('\\')) + 1
  return `${template.slice(0, cut)}{output}_${template.slice(cut)}`
}
