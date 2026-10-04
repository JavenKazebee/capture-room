<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { toast } from 'vue-sonner'
import { Film, FolderOpen, Loader2 } from '@lucide/vue'
import { errorMessage } from '@/composables/useApi'
import { useCapacityStore } from '@/stores/capacity'
import { useNodesStore } from '@/stores/nodes'
import { blankLeg, presetLegs, usePresetsStore } from '@/stores/presets'
import { useSourcesStore } from '@/stores/sources'
import type { ConfiguredSourceDto } from '@/types/generated/ConfiguredSourceDto'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'
import FileBrowseDialog from '@/components/sources/FileBrowseDialog.vue'

/** Start a benchmark on one node. Mount it with `v-if`. */
const props = defineProps<{ nodeId: string }>()
const emit = defineEmits<{ close: [] }>()

const capacity = useCapacityStore()
const nodes = useNodesStore()
const presets = usePresetsStore()
const sources = useSourcesStore()

const presetId = ref('default')
const presetOptions = computed(() => [
  { value: 'default', label: 'H.264 (default)' },
  ...presets.presets.map((p) => ({ value: p.id, label: p.name })),
])

const mediaPath = ref('')
const browsing = ref(false)
/** The node's file sources, offered as footage. */
const fileSources = ref<(ConfiguredSourceDto & { path: string })[]>([])

const maxFeeds = ref(64)
const stepSecs = ref(20)
const threshold = ref(0.5)

const saving = ref(false)
const error = ref<string | null>(null)

onMounted(async () => {
  const configs = await sources.configs(props.nodeId).catch(() => [])
  fileSources.value = configs.flatMap((c) => (c.config.type === 'file' ? [{ ...c, path: c.config.path }] : []))
  if (!mediaPath.value && fileSources.value[0]) mediaPath.value = fileSources.value[0].path
})

/**
 * Rough length of a run that finds its answer below the limit: a quick check
 * per doubling, then about as many full-length steps to narrow down, plus
 * warm-ups and starting feeds.
 */
const minutes = computed(() => {
  const doublings = Math.ceil(Math.log2(Math.max(2, maxFeeds.value))) + 1
  const secs = doublings * (5 + 8) + doublings * (stepSecs.value + 8)
  return Math.max(1, Math.round(secs / 60))
})

async function start() {
  error.value = null
  if (!mediaPath.value.trim()) {
    error.value = 'Pick the footage to play.'
    return
  }
  const preset = presets.presets.find((p) => p.id === presetId.value) ?? null
  saving.value = true
  try {
    await capacity.start(props.nodeId, {
      preset_id: preset?.id ?? null,
      preset_name: preset?.name ?? null,
      outputs: preset ? presetLegs(preset) : [blankLeg()],
      media_path: mediaPath.value.trim(),
      max_feeds: maxFeeds.value,
      step_secs: stepSecs.value,
      drop_threshold_pct: threshold.value,
    })
    toast.success(`Benchmarking ${nodes.nameOf(props.nodeId)}`)
    emit('close')
  } catch (e) {
    error.value = errorMessage(e, "Couldn't start the benchmark")
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <Dialog :open="true" @update:open="(v) => !v && emit('close')">
    <DialogContent class="sm:max-w-xl gap-4 max-h-[90vh] overflow-y-auto">
      <DialogHeader>
        <DialogTitle>Benchmark {{ nodes.nameOf(nodeId) }}</DialogTitle>
        <DialogDescription>
          Records more and more copies of the footage with a preset's outputs until frames drop, to find how many
          feeds this node can record.
        </DialogDescription>
      </DialogHeader>

      <form class="grid grid-cols-3 gap-3" @submit.prevent="start">
        <FormField label="Preset" class="col-span-3">
          <OptionSelect v-model="presetId" :options="presetOptions" />
        </FormField>

        <FormField label="Footage" class="col-span-3">
          <div class="flex gap-1.5">
            <Input v-model="mediaPath" class="font-mono text-[0.6875rem]" placeholder="/path/on/the/node/clip.mov" />
            <Button type="button" variant="outline" class="gap-1.5" @click="browsing = true">
              <FolderOpen class="size-3.5" /> Browse
            </Button>
          </div>
        </FormField>
        <div v-if="fileSources.length" class="col-span-3 -mt-1 flex flex-wrap gap-1">
          <Button
            v-for="f in fileSources"
            :key="f.id"
            type="button"
            variant="outline"
            size="sm"
            class="h-6 gap-1 px-2 text-xs"
            :class="mediaPath === f.path && 'border-primary text-primary'"
            :title="f.path"
            @click="mediaPath = f.path"
          >
            <Film class="size-3" /> {{ f.name }}
          </Button>
        </div>
        <p class="col-span-3 text-xs text-muted-foreground">
          Use real footage at the format your sources send: test patterns compress unrealistically, and results only
          count for recordings in the same format.
        </p>

        <FormField label="Feed limit">
          <Input v-model.number="maxFeeds" type="number" min="1" max="128" class="num" />
        </FormField>
        <FormField label="Seconds per step">
          <Input v-model.number="stepSecs" type="number" min="5" max="300" class="num" />
        </FormField>
        <FormField label="Drop limit (%)">
          <Input v-model.number="threshold" type="number" min="0.01" max="10" step="0.1" class="num" />
        </FormField>

        <div class="col-span-3 space-y-1.5 rounded-md border border-border bg-muted/40 p-3 text-xs text-muted-foreground">
          <p>
            Doubles the feeds with quick checks until frames drop, then narrows down with
            <span class="num">{{ stepSecs }}</span> s steps to the most feeds that keep within
            <span class="num">{{ threshold }}%</span> dropped frames, up to <span class="num">{{ maxFeeds }}</span>.
            Usually about <span class="num">{{ minutes }}</span> min.
          </p>
          <p>
            Files are written beside where the preset records, so the same disks are measured, and deleted afterwards.
            Starting a recording on this node cancels the benchmark.
          </p>
        </div>
        <button type="submit" hidden />
      </form>

      <DialogFooter class="items-center">
        <p v-if="error" class="mr-auto text-xs text-destructive">{{ error }}</p>
        <Button variant="outline" :disabled="saving" @click="emit('close')">Cancel</Button>
        <Button :disabled="saving" class="gap-1.5" @click="start">
          <Loader2 v-if="saving" class="size-3.5 animate-spin" />
          Start
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>

  <FileBrowseDialog
    v-if="browsing"
    :node-id="nodeId"
    :start="mediaPath || undefined"
    @close="browsing = false"
    @select="(p) => (mediaPath = p)"
  />
</template>
