<script setup lang="ts">
import { computed, nextTick, ref, type ComponentPublicInstance } from 'vue'
import { useRouter } from 'vue-router'
import { Check, Pencil, Radio, Trash2, Video, X } from '@lucide/vue'
import { formatUptime } from '@/lib/format'
import { notifyError } from '@/lib/notify'
import { useNodesStore } from '@/stores/nodes'
import { useSourcesStore } from '@/stores/sources'
import { useRecordingsStore } from '@/stores/recordings'
import { useStorageStore } from '@/stores/storage'
import { useRecordDeskStore } from '@/stores/recordDesk'
import type { NodeDto } from '@/types/generated/NodeDto'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import CopyButton from '@/components/common/CopyButton.vue'
import StatusDot from '@/components/common/StatusDot.vue'
import StorageVolumeBar from '@/components/common/StorageVolumeBar.vue'

/**
 * One node: identity, activity, storage. Sections keep their height whether the
 * node is reachable or not, so a node going offline doesn't reflow the grid.
 */
const props = defineProps<{ node: NodeDto }>()
const emit = defineEmits<{ remove: [] }>()

const nodes = useNodesStore()
const sources = useSourcesStore()
const recordings = useRecordingsStore()
const storage = useStorageStore()
const desk = useRecordDeskStore()
const router = useRouter()

const isController = computed(() => props.node.is_self && nodes.isController)
const canRemove = computed(() => !props.node.is_self && props.node.manual && nodes.isController)

const nodeSources = computed(() => sources.sources.filter((s) => s.node_id === props.node.id))
const stats = computed(() => ({
  total: nodeSources.value.length,
  connected: nodeSources.value.filter((s) => s.connected && !s.error).length,
  failed: nodeSources.value.filter((s) => s.error).length,
  recording: recordings.activeSessions.filter((s) => s.node_id === props.node.id).length,
}))

/** `undefined` while loading, `null` if it failed. */
const volumes = computed(() => storage.volumes.get(props.node.id))

function openInRecord() {
  desk.nodeFilter = props.node.id
  router.push('/record')
}

// ── Rename ────────────────────────────────────────────────────────────────────

const editing = ref(false)
const draft = ref('')
const saving = ref(false)
const input = ref<ComponentPublicInstance | null>(null)

async function startRename() {
  draft.value = props.node.name
  editing.value = true
  await nextTick()
  const el = (input.value?.$el ?? null) as HTMLInputElement | null
  el?.focus()
  el?.select()
}

async function saveRename() {
  const name = draft.value.trim()
  if (!name || name === props.node.name) {
    editing.value = false
    return
  }
  saving.value = true
  try {
    await nodes.rename(props.node.id, name)
    editing.value = false
  } catch (e) {
    notifyError(`Couldn't rename ${props.node.name}`, e, props.node.id)
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <article class="flex flex-col rounded-lg border border-border bg-card">
    <!-- Identity -->
    <header class="px-4 pt-3 pb-2.5">
      <div class="flex items-center gap-2 h-7">
        <StatusDot :status="node.healthy ? 'ok' : 'error'" />
        <form v-if="editing" class="flex flex-1 min-w-0 items-center gap-1" @submit.prevent="saveRename">
          <Input
            ref="input"
            v-model="draft"
            class="h-7 text-sm font-medium"
            :disabled="saving"
            @keydown.esc="editing = false"
          />
          <Button type="submit" variant="ghost" size="icon" class="size-7" :disabled="saving" title="Save">
            <Check class="size-3.5" />
          </Button>
          <Button type="button" variant="ghost" size="icon" class="size-7" :disabled="saving" title="Cancel" @click="editing = false">
            <X class="size-3.5" />
          </Button>
        </form>
        <template v-else>
          <h2 class="text-sm font-semibold truncate" :title="node.name">{{ node.name }}</h2>
          <Button
            v-if="node.healthy"
            variant="ghost"
            size="icon"
            class="size-6 text-muted-foreground"
            title="Rename"
            @click="startRename"
          >
            <Pencil class="size-3" />
          </Button>
          <div class="flex-1" />
          <Badge v-if="isController" variant="default">Controller</Badge>
          <Badge v-if="node.is_self" variant="outline">This machine</Badge>
          <Badge v-else variant="secondary">{{ node.manual ? 'Added by address' : 'Discovered' }}</Badge>
        </template>
      </div>
      <div class="mt-0.5 pl-4 flex items-center gap-1 h-5 text-xs text-muted-foreground min-w-0">
        <span class="num truncate">{{ node.url || 'local' }}</span>
        <CopyButton v-if="node.url" :value="node.url" />
        <template v-if="node.healthy">
          <span>·</span><span class="num shrink-0">v{{ node.version }}</span>
          <span>·</span><span class="shrink-0">up <span class="num">{{ formatUptime(node.uptime_secs) }}</span></span>
        </template>
        <template v-else>
          <span>·</span><span class="text-destructive shrink-0">Unreachable</span>
        </template>
      </div>
    </header>

    <!-- Activity -->
    <dl class="grid grid-cols-3 border-y border-border divide-x divide-border text-xs">
      <div class="px-4 py-2">
        <dt class="text-muted-foreground">Sources</dt>
        <dd class="num text-sm mt-0.5">
          <template v-if="node.healthy">{{ stats.connected }}<span class="text-muted-foreground">/{{ stats.total }}</span></template>
          <span v-else class="text-muted-foreground">—</span>
        </dd>
      </div>
      <div class="px-4 py-2">
        <dt class="text-muted-foreground">Recording</dt>
        <dd class="num text-sm mt-0.5 flex items-center gap-1.5">
          <template v-if="node.healthy">
            <StatusDot v-if="stats.recording" status="tally" />
            <span :class="stats.recording ? 'text-tally font-semibold' : 'text-muted-foreground'">{{ stats.recording }}</span>
          </template>
          <span v-else class="text-muted-foreground">—</span>
        </dd>
      </div>
      <div class="px-4 py-2">
        <dt class="text-muted-foreground">Failed</dt>
        <dd class="num text-sm mt-0.5">
          <span v-if="node.healthy" :class="stats.failed ? 'text-destructive' : 'text-muted-foreground'">{{ stats.failed }}</span>
          <span v-else class="text-muted-foreground">—</span>
        </dd>
      </div>
    </dl>

    <!-- Storage -->
    <section class="px-4 py-3 space-y-2">
      <h3 class="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">Storage</h3>
      <p v-if="!node.healthy" class="text-xs text-muted-foreground h-[1.625rem] flex items-center">Not available while unreachable.</p>
      <p v-else-if="volumes === undefined" class="text-xs text-muted-foreground h-[1.625rem] flex items-center">Loading…</p>
      <p v-else-if="volumes === null" class="text-xs text-destructive h-[1.625rem] flex items-center">Couldn't read storage.</p>
      <StorageVolumeBar v-for="v in node.healthy ? volumes ?? [] : []" :key="v.mount_point" :volume="v" />
    </section>

    <!-- Capacity (benchmark estimator) goes here, between storage and actions. -->

    <footer class="mt-auto flex items-center gap-1.5 px-3 py-2 border-t border-border">
      <Button variant="ghost" size="sm" class="h-7 gap-1.5 text-xs" :disabled="!node.healthy" @click="openInRecord">
        <Video class="size-3.5" /> Open in Record
      </Button>
      <Button variant="ghost" size="sm" class="h-7 gap-1.5 text-xs" :disabled="!node.healthy" @click="router.push('/setup/sources')">
        <Radio class="size-3.5" /> Sources
      </Button>
      <div class="flex-1" />
      <Button
        v-if="canRemove"
        variant="ghost"
        size="sm"
        class="h-7 gap-1.5 text-xs text-muted-foreground hover:text-destructive"
        @click="emit('remove')"
      >
        <Trash2 class="size-3.5" /> Remove
      </Button>
    </footer>
  </article>
</template>
