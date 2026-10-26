<script setup lang="ts">
import { computed, nextTick, ref, type ComponentPublicInstance } from 'vue'
import { RouterLink, useRouter } from 'vue-router'
import { toast } from 'vue-sonner'
import { Check, Clock, Pencil, Radio, Trash2, TriangleAlert, Video, X } from '@lucide/vue'
import { clockSummary } from '@/lib/clock'
import { formatFingerprint, formatUptime } from '@/lib/format'
import { notifyError } from '@/lib/notify'
import { olderThan } from '@/lib/version'
import { useNodesStore } from '@/stores/nodes'
import { useSourcesStore } from '@/stores/sources'
import { useRecordingsStore } from '@/stores/recordings'
import { useStorageStore } from '@/stores/storage'
import { useRecordDeskStore } from '@/stores/recordDesk'
import { useChecksStore } from '@/stores/checks'
import type { NodeDto } from '@/types/generated/NodeDto'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import CopyButton from '@/components/common/CopyButton.vue'
import StatusDot from '@/components/common/StatusDot.vue'
import StorageVolumeBar from '@/components/common/StorageVolumeBar.vue'
import NodeCapacity from './NodeCapacity.vue'
import NodeUpdate from './NodeUpdate.vue'

/**
 * One node: identity, activity, storage, capacity. Sections keep their height whether the
 * node is reachable or not, so a node going offline doesn't reflow the grid.
 */
const props = defineProps<{ node: NodeDto }>()
const emit = defineEmits<{ remove: [] }>()

const nodes = useNodesStore()
const sources = useSourcesStore()
const recordings = useRecordingsStore()
const storage = useStorageStore()
const desk = useRecordDeskStore()
const checks = useChecksStore()
const router = useRouter()

const isController = computed(() => props.node.is_self && nodes.isController)
const canRemove = computed(() => !props.node.is_self && nodes.isController)

const nodeSources = computed(() => sources.sources.filter((s) => s.node_id === props.node.id))
const stats = computed(() => ({
  total: nodeSources.value.length,
  connected: nodeSources.value.filter((s) => s.connected && !s.error).length,
  failed: nodeSources.value.filter((s) => s.error).length,
  recording: recordings.activeSessions.filter((s) => s.node_id === props.node.id).length,
}))

const clock = computed(() => clockSummary(props.node.clock))

/** Why it's unreachable: a few words for the header, then what to do about it. */
const down = computed(() => {
  const u = props.node.unreachable
  switch (u?.kind) {
    case 'access-denied':
      return {
        label: 'Not paired',
        detail: 'Uses another access token. Pair it with the pairing code from its menu bar.',
      }
    case 'key-changed':
      return {
        label: 'Key changed',
        detail: `Holds key ${formatFingerprint(u.fingerprint, true)}, not the approved one. If it was reinstalled, check the key in its Settings, then remove it and add it again.`,
      }
    case 'other-node':
      return { label: 'Another node here', detail: `${u.name} answers at this address now. Remove this node and add it at its new address.` }
    case 'failing':
      return { label: 'Not answering', detail: `Answers, but its check failed: ${u.error}` }
    case 'offline':
      return { label: 'Offline', detail: 'Nothing answers at this address: check it’s on and on the network.' }
    case 'other-version':
      return {
        label: `On v${u.version}`,
        detail: olderThan(u.version, nodes.version)
          ? `Runs ${u.version}, older than this controller’s ${nodes.version}. Update it to use it.`
          : `Runs ${u.version}, newer than this controller’s ${nodes.version}. Update the controller to use it.`,
      }
    default:
      return { label: 'Unreachable', detail: 'Not available while unreachable.' }
  }
})
const problems = computed(() => (props.node.healthy ? checks.problems(props.node.id) : 0))

/** `undefined` while loading, `null` if it failed. */
const volumes = computed(() => storage.volumes.get(props.node.id))

function openInRecord() {
  desk.nodeFilter = props.node.id
  router.push('/record')
}

/** Pairing a node that refuses this controller's token. */
const pairCode = ref('')
const pairing = ref(false)

async function pair() {
  const code = pairCode.value.trim()
  if (!code || pairing.value) return
  pairing.value = true
  try {
    await nodes.pair(props.node.id, code)
    pairCode.value = ''
    toast.success(`Paired ${props.node.name}`)
  } catch (e) {
    notifyError(`Couldn't pair ${props.node.name}`, e, props.node.id)
  } finally {
    pairing.value = false
  }
}

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
          <Badge v-if="problems" as-child variant="destructive">
            <RouterLink :to="`/setup/checks#node-${node.id}`" title="See what's missing in Checks">
              <TriangleAlert /> {{ problems }} {{ problems === 1 ? 'problem' : 'problems' }}
            </RouterLink>
          </Badge>
          <Badge v-if="isController" variant="default">Controller</Badge>
          <Badge v-if="node.is_self" variant="outline">This machine</Badge>
          <Badge
            v-if="node.fingerprint"
            variant="secondary"
            class="num"
            :title="`TLS key ${formatFingerprint(node.fingerprint)}`"
          >
            Key {{ formatFingerprint(node.fingerprint, true) }}
          </Badge>
          <Badge v-else variant="secondary" title="Approved before TLS: its key is pinned when it next answers">
            Key not pinned
          </Badge>
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
          <span>·</span><span class="text-destructive shrink-0">{{ down.label }}</span>
        </template>
      </div>
      <div class="pl-4 flex items-center gap-1.5 h-5 text-xs text-muted-foreground min-w-0" :title="node.healthy ? clock.title : undefined">
        <Clock class="size-3 shrink-0" />
        <template v-if="node.healthy">
          <span class="truncate">{{ clock.label }}</span>
          <template v-if="clock.detail">
            <span>·</span>
            <StatusDot v-if="clock.status !== 'off'" :status="clock.status" />
            <span class="num shrink-0" :class="{ 'text-warning': clock.status === 'warn', 'text-destructive': clock.status === 'error' }">{{ clock.detail }}</span>
          </template>
        </template>
        <span v-else>—</span>
      </div>
    </header>

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

    <section class="px-4 py-3 space-y-2">
      <h3 class="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">Storage</h3>
      <p
        v-if="!node.healthy"
        class="text-xs min-h-[1.625rem] flex items-center line-clamp-2 break-words"
        :class="node.unreachable && node.unreachable.kind !== 'offline' ? 'text-destructive' : 'text-muted-foreground'"
        :title="down.detail"
      >
        {{ down.detail }}
      </p>
      <form
        v-if="!node.healthy && node.unreachable?.kind === 'access-denied' && nodes.isController"
        class="flex gap-2"
        @submit.prevent="pair"
      >
        <Input
          v-model="pairCode"
          placeholder="Pairing code"
          aria-label="Pairing code"
          class="h-7 font-mono text-xs uppercase"
          maxlength="12"
          spellcheck="false"
          autocomplete="off"
          :disabled="pairing"
        />
        <Button type="submit" size="sm" class="h-7 text-xs" :disabled="pairing || !pairCode.trim()">
          {{ pairing ? 'Pairing…' : 'Pair' }}
        </Button>
      </form>
      <p v-else-if="volumes === undefined" class="text-xs text-muted-foreground h-[1.625rem] flex items-center">Loading…</p>
      <p v-else-if="volumes === null" class="text-xs text-destructive h-[1.625rem] flex items-center">Couldn't read storage.</p>
      <StorageVolumeBar v-for="v in node.healthy ? volumes ?? [] : []" :key="v.mount_point" :volume="v" />
    </section>

    <NodeCapacity :node="node" :recording="stats.recording" />

    <footer class="mt-auto flex items-center gap-1.5 px-3 py-2 border-t border-border">
      <Button variant="ghost" size="sm" class="h-7 gap-1.5 text-xs" :disabled="!node.healthy" @click="openInRecord">
        <Video class="size-3.5" /> Open in Record
      </Button>
      <Button variant="ghost" size="sm" class="h-7 gap-1.5 text-xs" :disabled="!node.healthy" @click="router.push('/setup/sources')">
        <Radio class="size-3.5" /> Sources
      </Button>
      <div class="flex-1" />
      <NodeUpdate :node="node" />
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
