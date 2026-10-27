<script setup lang="ts">
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { useEventListener } from '@vueuse/core'
import { toast } from 'vue-sonner'
import { Copy, Loader2, Lock, Plus, RotateCcw, Search, Trash2 } from '@lucide/vue'
import { usePresetsStore, blankLeg, DEFAULT_PRESET } from '@/stores/presets'
import { dateTime, plural } from '@/lib/format'
import { useSourcesStore } from '@/stores/sources'
import { useNodesStore } from '@/stores/nodes'
import { useRecordDeskStore } from '@/stores/recordDesk'
import { errorMessage } from '@/composables/useApi'
import { useQueryAction } from '@/composables/useQueryAction'
import { useUnsavedGuard } from '@/composables/useUnsavedGuard'
import { notifyError } from '@/lib/notify'
import { CODECS, clashingLegs, legProblems } from '@/lib/codecs'
import { advancedProblems } from '@/lib/advanced'
import { shortcut } from '@/lib/keys'
import type { PresetDto } from '@/types/generated/PresetDto'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import PageHeader from '@/components/common/PageHeader.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import LoadError from '@/components/common/LoadError.vue'
import OutputCard from '@/components/presets/OutputCard.vue'

const store = usePresetsStore()
const sources = useSourcesStore()
const nodes = useNodesStore()
const desk = useRecordDeskStore()

const query = ref('')
const listed = computed(() => {
  const q = query.value.trim().toLowerCase()
  return [...store.presets]
    .filter((p) => !q || p.name.toLowerCase().includes(q))
    .sort((a, b) => a.name.localeCompare(b.name))
})

function codecsOf(outputs: PresetOutputInput[]) {
  return [...new Set(outputs.map((o) => CODECS[o.codec]))].join(' + ')
}

/** How many feeds in Record currently have this preset chosen. */
function usedBy(id: string) {
  return sources.sources.filter((s) => desk.presetIdOf(s.key) === id).length
}

/** A preset id, `default` (the built-in, read-only), or `new`. */
const selectedId = ref<string | null>(null)
const draftName = ref('')
const draftLegs = ref<PresetOutputInput[]>([])
/** Stable keys for the output cards, moved and spliced alongside `draftLegs`. */
const legKeys = ref<number[]>([])
let nextKey = 0
const original = ref('')
const saving = ref(false)
const serverError = ref<string | null>(null)

const selected = computed(() => store.presets.find((p) => p.id === selectedId.value) ?? null)
const isBuiltIn = computed(() => selectedId.value === DEFAULT_PRESET.id)
const snapshot = () => JSON.stringify({ name: draftName.value, legs: draftLegs.value })
const dirty = computed(() => !isBuiltIn.value && selectedId.value !== null && snapshot() !== original.value)

/** Record feeds that asked for this draft ("New preset…"); it's applied to them once saved. */
const pendingFor = ref<string[]>([])

function load(id: string, name: string, legs: PresetOutputInput[]) {
  // Leaving the draft for another preset drops the request.
  if (id !== 'new') pendingFor.value = []
  selectedId.value = id
  draftName.value = name
  draftLegs.value = legs.map((l) => ({ ...l }))
  legKeys.value = legs.map(() => nextKey++)
  original.value = id === 'new' ? '' : snapshot()
  serverError.value = null
}

function loadPreset(p: PresetDto) {
  load(p.id, p.name, p.outputs.length ? p.outputs : [blankLeg()])
}

// Switching away from unsaved changes, or leaving the page, asks first.
const { confirmOpen: confirmSwitch, guarded, discard: discardAndSwitch } = useUnsavedGuard(dirty)

function select(id: string) {
  if (id === selectedId.value) return
  guarded(() => {
    if (id === DEFAULT_PRESET.id) load(DEFAULT_PRESET.id, DEFAULT_PRESET.name, [blankLeg()])
    else {
      const p = store.presets.find((p) => p.id === id)
      if (p) loadPreset(p)
    }
  })
}

const nameInput = ref<{ $el: HTMLInputElement } | null>(null)

function create() {
  guarded(() => {
    load('new', '', [blankLeg()])
    // A new preset's first job is a name.
    nextTick(() => nameInput.value?.$el.focus())
  })
}

function duplicate() {
  const name = `${draftName.value || 'Preset'} copy`
  const legs = draftLegs.value
  guarded(() => load('new', name, legs))
  draftName.value = name
}

function revert() {
  if (selected.value) loadPreset(selected.value)
  else if (selectedId.value === 'new') load('new', '', [blankLeg()])
}

function addOutput() {
  const n = draftLegs.value.length + 1
  // {output} keeps a second output from writing the first one's file.
  draftLegs.value.push({ ...blankLeg(), name: `Output ${n}`, path_template: '~/capture-room/{date}/{source}_{output}_{datetime}.{ext}' })
  legKeys.value.push(nextKey++)
}

function duplicateOutput(i: number) {
  const leg = draftLegs.value[i]!
  draftLegs.value.splice(i + 1, 0, { ...leg, name: `${leg.name} copy`, preview: false })
  legKeys.value.splice(i + 1, 0, nextKey++)
}

/** Only one output is the preview: marking one unmarks the others. */
function setPreview(i: number) {
  draftLegs.value.forEach((leg, j) => {
    if (j !== i && leg.preview) draftLegs.value[j] = { ...leg, preview: false }
  })
}

function removeOutput(i: number) {
  draftLegs.value.splice(i, 1)
  legKeys.value.splice(i, 1)
}

function moveOutput(i: number, dir: -1 | 1) {
  for (const list of [draftLegs.value, legKeys.value] as unknown[][]) {
    ;[list[i], list[i + dir]] = [list[i + dir], list[i]]
  }
}

const clashes = computed(() => clashingLegs(draftLegs.value))
/** What's blocking Save, in words: the name, then each output with a problem. */
const problems = computed(() => {
  const list: string[] = []
  if (!draftName.value.trim()) list.push('Name the preset')
  draftLegs.value.forEach((l, i) => {
    if (clashes.value.has(i) || Object.keys(legProblems(l)).length || Object.keys(advancedProblems(l)).length) {
      list.push(`Fix ${l.name || `output ${i + 1}`}`)
    }
  })
  return list
})
const problemCount = computed(() => problems.value.length)

const preview = computed(() => ({
  source: sources.sources[0]?.id ?? 'cam1',
  sourceName: sources.sources[0]?.display_name ?? 'Camera 1',
  node: nodes.self?.node_name ?? 'node',
  preset: draftName.value.trim(),
}))

async function save() {
  if (saving.value || !dirty.value || problemCount.value) return
  saving.value = true
  serverError.value = null
  try {
    const payload = { name: draftName.value.trim(), outputs: draftLegs.value }
    const p = selectedId.value === 'new' ? await store.create(payload) : await store.update(selectedId.value!, payload)
    // Settle the editor on the saved preset first: a failure after this
    // must not leave the draft open, where saving again would duplicate it.
    const feedKeys = pendingFor.value
    const feeds = feedKeys.length
    loadPreset(p)
    if (feeds) desk.setPresetMany(feedKeys, p.id)
    toast.success(feeds ? `Saved ${p.name} · applied to ${plural(feeds, 'feed')}` : `Saved ${p.name}`)
  } catch (e) {
    serverError.value = errorMessage(e, 'Save failed.')
  } finally {
    saving.value = false
  }
}

useEventListener('keydown', (e: KeyboardEvent) => {
  if (e.key.toLowerCase() === 's' && (e.ctrlKey || e.metaKey)) {
    e.preventDefault()
    save()
  }
})

const confirmDelete = ref(false)
async function destroy() {
  const p = selected.value
  if (!p) return
  try {
    await store.remove(p.id)
    toast.success(`Deleted ${p.name}`)
    selectedId.value = null
    pickInitial()
  } catch (e) {
    notifyError(`Delete failed: ${p.name}`, e)
  }
}

function pickInitial() {
  const first = listed.value[0]
  if (first) loadPreset(first)
  else load(DEFAULT_PRESET.id, DEFAULT_PRESET.name, [blankLeg()])
}

const ready = ref(false)
onMounted(async () => {
  if (!store.presets.length) await store.load()
  pickInitial()
  ready.value = true
})

// "New preset…" from a Record picker lands here with ?new=1, and ?for= the feeds to apply it to.
useQueryAction(['new', 'for'], (q) => {
  if (!q.new) return
  create()
  pendingFor.value = ([] as unknown[]).concat(q.for ?? []).filter((k): k is string => typeof k === 'string')
}, { when: () => ready.value })

// A preset updated elsewhere refreshes the editor unless there are local edits.
watch(selected, (p) => p && !dirty.value && p.id === selectedId.value && loadPreset(p))
</script>

<template>
  <div class="h-full flex flex-col min-h-0">
    <PageHeader title="Presets" :count="store.presets.length">
      <template #actions>
        <Button size="sm" class="h-7 gap-1.5 text-xs" @click="create"><Plus class="size-3.5" /> New preset</Button>
      </template>
    </PageHeader>

    <div class="flex-1 min-h-0 flex">
      <aside class="w-64 shrink-0 border-r border-border flex flex-col min-h-0">
        <div class="p-2 border-b border-border relative">
          <Search class="absolute left-4 top-1/2 -translate-y-1/2 size-3.5 text-muted-foreground" />
          <Input v-model="query" placeholder="Filter presets…" class="h-7 pl-7 text-xs" />
        </div>
        <LoadError :error="store.error" :retry="store.load" class="px-3 py-2 border-b border-border" />
        <nav class="flex-1 overflow-y-auto p-1.5 space-y-0.5">
          <button
            class="preset-item"
            :data-active="selectedId === DEFAULT_PRESET.id || undefined"
            @click="select(DEFAULT_PRESET.id)"
          >
            <span class="flex items-center gap-1.5 text-sm font-medium"><Lock class="size-3 text-muted-foreground" /> {{ DEFAULT_PRESET.name }}</span>
            <span class="text-[11px] text-muted-foreground">Built in · 1 output · H.264</span>
          </button>
          <button
            v-if="selectedId === 'new'"
            class="preset-item"
            data-active
          >
            <span class="text-sm font-medium italic">{{ draftName || 'New preset' }}</span>
            <span class="text-[11px] text-muted-foreground">Unsaved</span>
          </button>
          <button
            v-for="p in listed"
            :key="p.id"
            class="preset-item"
            :data-active="selectedId === p.id || undefined"
            @click="select(p.id)"
          >
            <span class="text-sm font-medium truncate w-full">
              {{ p.name }}
              <span v-if="selectedId === p.id && dirty" class="text-primary" title="Unsaved changes">•</span>
            </span>
            <span class="text-[11px] text-muted-foreground truncate w-full">
              {{ plural(p.outputs.length, 'output') }} · {{ codecsOf(p.outputs) }}
              <template v-if="usedBy(p.id)"> · on {{ plural(usedBy(p.id), 'feed') }}</template>
            </span>
          </button>
          <p v-if="!listed.length && query" class="px-2 py-3 text-xs text-muted-foreground">No presets match.</p>
        </nav>
      </aside>

      <section v-if="selectedId" class="flex-1 min-w-0 flex flex-col min-h-0">
        <div class="shrink-0 flex flex-wrap items-center gap-3 px-4 py-2.5 border-b border-border">
          <Input
            ref="nameInput"
            v-model="draftName"
            placeholder="Preset name"
            class="h-8 max-w-sm text-sm font-semibold"
            :disabled="isBuiltIn"
            :aria-invalid="!isBuiltIn && !draftName.trim()"
          />
          <span v-if="selected" class="text-[11px] text-muted-foreground">
            v{{ selected.version }} · updated {{ dateTime(selected.updated_at, { year: true }) }}
          </span>
          <div class="flex-1" />
          <Button variant="outline" size="sm" class="h-7 gap-1.5 text-xs" @click="duplicate">
            <Copy class="size-3.5" /> {{ isBuiltIn ? 'Copy as new preset' : 'Duplicate' }}
          </Button>
          <template v-if="!isBuiltIn">
            <Button v-if="selected" variant="destructive" size="sm" class="h-7 gap-1.5 text-xs" @click="confirmDelete = true">
              <Trash2 class="size-3.5" /> Delete
            </Button>
            <Button variant="outline" size="sm" class="h-7 gap-1.5 text-xs" :disabled="!dirty" @click="revert">
              <RotateCcw class="size-3.5" /> Revert
            </Button>
            <span v-if="problemCount" class="text-xs text-destructive">{{ problems.join(' · ') }}</span>
            <Button size="sm" class="h-7 gap-1.5 text-xs" :disabled="!dirty || !!problemCount || saving" :title="problemCount ? problems.join('\n') : `Save (${shortcut('S')})`" @click="save">
              <Loader2 v-if="saving" class="size-3.5 animate-spin" /> Save
            </Button>
          </template>
        </div>

        <div class="flex-1 overflow-y-auto overflow-x-hidden p-4 space-y-3">
          <p v-if="isBuiltIn" class="text-xs text-muted-foreground rounded-md border border-border bg-muted/30 px-3 py-2">
            Used when a feed has no preset chosen. It can't be edited — copy it to make your own.
          </p>
          <p v-if="serverError" class="text-xs text-destructive rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2">
            {{ serverError }}
          </p>

          <div class="flex items-center justify-between">
            <h2 class="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
              Outputs <span class="num font-normal">{{ draftLegs.length }}</span>
            </h2>
            <Button v-if="!isBuiltIn" variant="outline" size="sm" class="h-7 gap-1.5 text-xs" @click="addOutput">
              <Plus class="size-3.5" /> Add output
            </Button>
          </div>

          <fieldset :disabled="isBuiltIn" class="space-y-2" :class="isBuiltIn && 'opacity-80'">
            <OutputCard
              v-for="(_, i) in draftLegs"
              :key="legKeys[i]"
              v-model="draftLegs[i]!"
              :index="i"
              :count="draftLegs.length"
              :clash="clashes.has(i)"
              :preview="preview"
              @remove="removeOutput(i)"
              @duplicate="duplicateOutput(i)"
              @move="(d) => moveOutput(i, d)"
              @preview="setPreview(i)"
            />
          </fieldset>
          <p class="text-[11px] text-muted-foreground">
            Path previews use <span class="num">{{ preview.source }}</span> on
            <span class="num">{{ preview.node }}</span> at the current time. Every output records the same feed at once.
          </p>
        </div>
      </section>
    </div>

    <ConfirmDialog
      v-model:open="confirmDelete"
      :title="`Delete ${selected?.name}?`"
      :description="`Recordings already made are kept.${selected && usedBy(selected.id) ? ` ${usedBy(selected.id)} feed(s) using it will fall back to the default preset.` : ''}`"
      @confirm="destroy"
    />
    <ConfirmDialog
      v-model:open="confirmSwitch"
      title="Discard unsaved changes?"
      description="Your edits to this preset haven't been saved."
      confirm-label="Discard"
      @confirm="discardAndSwitch"
    />
  </div>
</template>

<style scoped>
@reference "@/style.css";
.preset-item {
  @apply w-full flex flex-col items-start gap-0.5 rounded-md px-2.5 py-1.5 text-left hover:bg-accent/60;
}
.preset-item[data-active] {
  @apply bg-primary/10 ring-1 ring-inset ring-primary/40;
}
</style>
