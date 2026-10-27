<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from 'vue'
import { useEventListener } from '@vueuse/core'
import { toast } from 'vue-sonner'
import { Copy, Loader2, Plus, RotateCcw, Trash2 } from '@lucide/vue'
import { usePresetsStore, blankLeg } from '@/stores/presets'
import { dateTime, plural } from '@/lib/format'
import { useSourcesStore } from '@/stores/sources'
import { useNodesStore } from '@/stores/nodes'
import { useRecordDeskStore } from '@/stores/recordDesk'
import { useSchedulesStore } from '@/stores/schedules'
import { errorMessage } from '@/composables/useApi'
import { usePresetDraft } from '@/composables/usePresetDraft'
import { useQueryAction } from '@/composables/useQueryAction'
import { notifyError } from '@/lib/notify'
import { shortcut } from '@/lib/keys'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import PageHeader from '@/components/common/PageHeader.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'
import OutputCard from '@/components/presets/OutputCard.vue'
import PresetList from '@/components/presets/PresetList.vue'

const store = usePresetsStore()
const sources = useSourcesStore()
const nodes = useNodesStore()
const desk = useRecordDeskStore()
const schedules = useSchedulesStore()

const sorted = computed(() => [...store.presets].sort((a, b) => a.name.localeCompare(b.name)))

/** How many feeds in Record currently have this preset chosen. */
function usedBy(id: string) {
  return sources.sources.filter((s) => desk.presetIdOf(s.key) === id).length
}

const draft = usePresetDraft()
const {
  selectedId,
  name: draftName,
  legs: draftLegs,
  legKeys,
  serverError,
  pendingFor,
  selected,
  isBuiltIn,
  dirty,
  confirmOpen: confirmSwitch,
  discard: discardAndSwitch,
  clashes,
  problems,
} = draft
/** Nodes that record with the open preset: its feeds in Record (and those a new one is for) and its schedules. */
const presetNodes = computed(() => {
  const id = selectedId.value
  const feeds = new Set(pendingFor.value)
  const ids = new Set(sources.sources.filter((s) => feeds.has(s.key) || desk.presetIdOf(s.key) === id).map((s) => s.node_id))
  for (const a of schedules.schedules.flatMap((s) => s.actions)) {
    if (a.kind === 'record' && a.preset_id === id) for (const t of a.targets) ids.add(t.node_id)
  }
  return [...ids]
})
const problemCount = computed(() => problems.value.length)
const saving = ref(false)

const nameInput = ref<{ $el: HTMLInputElement } | null>(null)

function create() {
  draft.guarded(() => {
    draft.load('new', '', [blankLeg()])
    // A new preset's first job is a name.
    nextTick(() => nameInput.value?.$el.focus())
  })
}

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
    draft.loadPreset(p)
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
  const first = sorted.value[0]
  if (first) draft.loadPreset(first)
  else draft.loadDefault()
}

const ready = ref(false)
onMounted(async () => {
  if (!store.presets.length) await store.load()
  if (!schedules.loaded) schedules.load()
  pickInitial()
  ready.value = true
})

// "New preset…" from a Record picker lands here with ?new=1, and ?for= the feeds to apply it to.
useQueryAction(['new', 'for'], (q) => {
  if (!q.new) return
  create()
  pendingFor.value = ([] as unknown[]).concat(q.for ?? []).filter((k): k is string => typeof k === 'string')
}, { when: () => ready.value })
</script>

<template>
  <div class="h-full flex flex-col min-h-0">
    <PageHeader title="Presets" :count="store.presets.length">
      <template #actions>
        <Button size="sm" class="h-7 gap-1.5 text-xs" @click="create"><Plus class="size-3.5" /> New preset</Button>
      </template>
    </PageHeader>

    <div class="flex-1 min-h-0 flex">
      <PresetList
        :presets="sorted"
        :selected-id="selectedId"
        :draft-name="draftName"
        :dirty="dirty"
        :used-by="usedBy"
        @select="draft.select"
      />

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
          <Button variant="outline" size="sm" class="h-7 gap-1.5 text-xs" @click="draft.duplicate">
            <Copy class="size-3.5" /> {{ isBuiltIn ? 'Copy as new preset' : 'Duplicate' }}
          </Button>
          <template v-if="!isBuiltIn">
            <Button v-if="selected" variant="destructive" size="sm" class="h-7 gap-1.5 text-xs" @click="confirmDelete = true">
              <Trash2 class="size-3.5" /> Delete
            </Button>
            <Button variant="outline" size="sm" class="h-7 gap-1.5 text-xs" :disabled="!dirty" @click="draft.revert">
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
            <Button v-if="!isBuiltIn" variant="outline" size="sm" class="h-7 gap-1.5 text-xs" @click="draft.addOutput">
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
              :node-ids="presetNodes"
              @remove="draft.removeOutput(i)"
              @duplicate="draft.duplicateOutput(i)"
              @move="(d) => draft.moveOutput(i, d)"
              @preview="draft.setPreview(i)"
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
