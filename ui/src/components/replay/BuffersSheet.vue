<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useSourcesStore, type Source } from '@/stores/sources'
import { useNodesStore } from '@/stores/nodes'
import { CODECS, useReplayStore } from '@/stores/replay'
import { notifyError } from '@/lib/notify'
import { formatBytes } from '@/lib/format'
import type { ReplayCodec } from '@/types/generated/ReplayCodec'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import OptionSelect from '@/components/OptionSelect.vue'
import StatusDot from '@/components/common/StatusDot.vue'

/**
 * Which sources keep a replay buffer, in what codec, and for how long.
 * Changes apply at once; a new codec starts the buffer afresh.
 */
const emit = defineEmits<{ close: [] }>()

const sources = useSourcesStore()
const nodes = useNodesStore()
const replay = useReplayStore()

const DEFAULT_MINUTES = 30

/** Every source but playout channels, grouped by node when there are several. */
const candidates = computed(() => sources.sources.filter((s) => s.source_type !== 'channel'))
const busy = ref<string | null>(null)

onMounted(() => {
  for (const n of new Set(candidates.value.map((s) => s.node_id))) replay.loadCodecs(n).catch(() => {})
})

function codecOptions(nodeId: string) {
  return (replay.codecs.get(nodeId) ?? []).map((c) => ({ value: c, label: CODECS[c].label }))
}

function stateOf(s: Source): { dot: 'ok' | 'off' | 'error'; label: string } {
  const b = replay.buffers.get(s.key)
  if (!b) return { dot: 'off', label: '' }
  if (b.error) return { dot: 'error', label: 'Not recording' }
  if (b.encoder) return { dot: 'ok', label: `Recording · ${formatBytes(b.bytes)}` }
  return { dot: 'off', label: 'Starting' }
}

async function apply(s: Source, what: string, fn: () => Promise<unknown>) {
  busy.value = s.key
  try {
    await fn()
  } catch (e) {
    notifyError(`${what} for ${s.display_name} failed`, e, s.node_id)
  } finally {
    busy.value = null
  }
}

async function toggle(s: Source, on: boolean) {
  if (!on) return apply(s, 'Removing the replay buffer', () => replay.removeBuffer(s.node_id, s.id))
  const codecs = await replay.loadCodecs(s.node_id).catch(() => [] as ReplayCodec[])
  if (!codecs.length) {
    notifyError(`${nodes.nameOf(s.node_id)} can't keep replay buffers`, new Error('It has none of the codecs installed.'))
    return
  }
  return apply(s, 'Starting a replay buffer', () =>
    replay.setBuffer(s.node_id, s.id, { codec: codecs[0], minutes: DEFAULT_MINUTES }),
  )
}

function setCodec(s: Source, codec: ReplayCodec) {
  const b = replay.buffers.get(s.key)
  if (!b || b.codec === codec) return
  apply(s, 'Changing the codec', () => replay.setBuffer(s.node_id, s.id, { codec, minutes: b.minutes }))
}

function setMinutes(s: Source, text: string | number) {
  const b = replay.buffers.get(s.key)
  const minutes = Math.round(Number(text))
  if (!b || !Number.isFinite(minutes) || minutes < 1 || minutes === b.minutes) return
  apply(s, 'Changing the length', () => replay.setBuffer(s.node_id, s.id, { codec: b.codec, minutes }))
}
</script>

<template>
  <Sheet :open="true" @update:open="(v) => !v && emit('close')">
    <SheetContent class="flex flex-col gap-0 p-0 w-full sm:max-w-lg">
      <SheetHeader class="border-b border-border">
        <SheetTitle>Replay buffers</SheetTitle>
        <SheetDescription>
          A source with a buffer records continuously on its node, keeping the last few minutes to replay. The
          oldest is deleted as it goes; nothing is kept when the node restarts.
        </SheetDescription>
      </SheetHeader>

      <div class="flex-1 overflow-y-auto divide-y divide-border">
        <p v-if="!candidates.length" class="p-4 text-sm text-muted-foreground">No sources yet: add some in Setup › Sources.</p>
        <div v-for="s in candidates" :key="s.key" class="p-4 flex flex-col gap-3">
          <div class="flex items-center gap-2 min-w-0">
            <Switch
              :model-value="replay.buffers.has(s.key)"
              :disabled="busy === s.key"
              :aria-label="`Keep a replay buffer for ${s.display_name}`"
              @update:model-value="(on: boolean) => toggle(s, on)"
            />
            <span class="text-sm font-medium truncate">{{ s.display_name }}</span>
            <span v-if="nodes.nodes.length > 1" class="text-[10px] uppercase tracking-wide text-muted-foreground shrink-0">
              {{ nodes.nameOf(s.node_id) }}
            </span>
            <span class="flex-1" />
            <span v-if="replay.buffers.get(s.key)" class="flex items-center gap-1.5 text-xs text-muted-foreground shrink-0">
              <StatusDot :status="stateOf(s).dot" />
              {{ stateOf(s).label }}
            </span>
          </div>
          <template v-if="replay.buffers.get(s.key)">
            <div class="grid grid-cols-[1fr_7rem] gap-3">
              <label class="flex flex-col gap-1">
                <span class="text-xs text-muted-foreground">Codec</span>
                <OptionSelect
                  :model-value="replay.buffers.get(s.key)!.codec"
                  :options="codecOptions(s.node_id)"
                  :disabled="busy === s.key"
                  class="h-8"
                  @update:model-value="(c) => setCodec(s, c)"
                />
              </label>
              <label class="flex flex-col gap-1">
                <span class="text-xs text-muted-foreground">Minutes kept</span>
                <Input
                  type="number"
                  min="1"
                  max="1440"
                  class="h-8 num"
                  :model-value="replay.buffers.get(s.key)!.minutes"
                  :disabled="busy === s.key"
                  @change="(e: Event) => setMinutes(s, (e.target as HTMLInputElement).value)"
                />
              </label>
            </div>
            <p class="text-xs text-muted-foreground">
              {{ CODECS[replay.buffers.get(s.key)!.codec].note }} A new codec starts the buffer afresh.
            </p>
            <p v-if="replay.buffers.get(s.key)!.error" class="text-xs text-destructive">
              {{ replay.buffers.get(s.key)!.error }}
            </p>
          </template>
        </div>
      </div>
    </SheetContent>
  </Sheet>
</template>
