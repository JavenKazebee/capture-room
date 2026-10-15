<script setup lang="ts">
import { computed, onScopeDispose, ref } from 'vue'
import { ExternalLink, Settings2, ShieldQuestion } from '@lucide/vue'
import { notifyError } from '@/lib/notify'
import { browserIsOn, isProblem, useChecksStore } from '@/stores/checks'
import type { CheckDto } from '@/types/generated/CheckDto'
import type { CheckGroup } from '@/types/generated/CheckGroup'
import type { NodeDto } from '@/types/generated/NodeDto'
import { Button } from '@/components/ui/button'
import CopyButton from '@/components/common/CopyButton.vue'
import StatusDot from '@/components/common/StatusDot.vue'

/** One node's checks, grouped, each with how to put it right. */
const props = defineProps<{ node: NodeDto }>()

const store = useChecksStore()

/** `undefined` while loading, `null` if it failed. */
const result = computed(() => store.byNode.get(props.node.id))
const probes = computed(() => store.reachability.get(props.node.id))
const problems = computed(() => store.problems(props.node.id))
const local = computed(() => browserIsOn(props.node))

const GROUPS: { key: CheckGroup; label: string }[] = [
  { key: 'permissions', label: 'Permissions' },
  { key: 'software', label: 'Software' },
  { key: 'storage', label: 'Storage' },
  { key: 'clock', label: 'Clock' },
  { key: 'network', label: 'Network' },
]

const groups = computed(() => {
  const checks = store.all(props.node.id)
  return GROUPS.map((g) => ({ ...g, checks: checks.filter((c) => c.group === g.key) })).filter(
    (g) => g.checks.length,
  )
})

function status(c: CheckDto): { dot: 'ok' | 'warn' | 'error' | 'off' | 'info'; label: string; class: string } {
  switch (c.status) {
    case 'ok':
      return { dot: 'ok', label: 'OK', class: 'text-muted-foreground' }
    case 'missing':
      return c.required
        ? { dot: 'error', label: 'Missing', class: 'text-destructive' }
        : { dot: 'warn', label: 'Not set up', class: 'text-warning' }
    case 'not_asked':
      return { dot: 'info', label: 'Not asked', class: 'text-info' }
    default:
      return { dot: 'off', label: 'Unknown', class: 'text-muted-foreground' }
  }
}

// ── Asking macOS ──────────────────────────────────────────────────────────────

const asking = ref<string | null>(null)
let disposed = false
onScopeDispose(() => (disposed = true))

/** Show the prompt on the node, then re-check until it's answered (or a minute passes). */
async function ask(c: CheckDto) {
  asking.value = c.id
  try {
    await store.request(props.node.id, c.id)
    // Stops when the page is left, too.
    for (let i = 0; i < 30 && !disposed; i++) {
      await new Promise((r) => setTimeout(r, 2000))
      if (disposed) break
      await store.loadForNode(props.node.id)
      const now = store.byNode.get(props.node.id)?.checks.find((x) => x.id === c.id)
      if (!now || now.status !== c.status) break
    }
  } catch (e) {
    notifyError(`Couldn't ask macOS for ${c.label}`, e, props.node.id)
  } finally {
    asking.value = null
  }
}
</script>

<template>
  <section :id="`node-${node.id}`" class="rounded-lg border border-border bg-card scroll-mt-4">
    <header class="flex items-center gap-2 px-4 py-2.5 border-b border-border">
      <StatusDot :status="result === undefined ? 'off' : result === null ? 'error' : problems ? 'error' : 'ok'" />
      <h2 class="text-sm font-semibold truncate">{{ node.name }}</h2>
      <span v-if="node.is_self" class="text-xs text-muted-foreground">This machine</span>
      <div class="flex-1" />
      <span v-if="result === undefined" class="text-xs text-muted-foreground">Checking…</span>
      <span v-else-if="result === null" class="text-xs text-destructive">Couldn't read its checks</span>
      <span v-else-if="problems" class="text-xs text-destructive font-medium">
        {{ problems }} {{ problems === 1 ? 'problem' : 'problems' }}
      </span>
      <span v-else class="text-xs text-muted-foreground">All good</span>
    </header>

    <p v-if="result?.permission_holder" class="px-4 pt-2.5 text-xs text-muted-foreground">
      macOS gives privacy permissions to the app that started the node:
      <span class="text-foreground font-medium">{{ result.permission_holder }}</span>. Started
      from another app, it needs them again.
    </p>
    <p v-if="result && !local" class="px-4 pt-1 text-xs text-muted-foreground">
      Open this page on {{ node.name }} itself (at localhost) for shortcuts to System Settings and the
      macOS prompts.
    </p>
    <p v-if="probes === null" class="px-4 pt-1 text-xs text-destructive">
      The controller couldn't probe this node's ports.
    </p>

    <div v-for="g in groups" :key="g.key" class="px-4 pt-3 pb-1 last:pb-3">
      <h3 class="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground mb-1">{{ g.label }}</h3>
      <ul class="divide-y divide-border">
        <li v-for="c in g.checks" :key="c.id" class="py-2 grid grid-cols-[auto_1fr_auto] gap-x-2.5 items-baseline">
          <StatusDot :status="status(c).dot" class="translate-y-px" />
          <div class="min-w-0">
            <div class="flex items-baseline gap-2 flex-wrap">
              <span class="text-sm font-medium">{{ c.label }}</span>
              <span class="text-xs text-muted-foreground">{{ c.needed_for }}</span>
            </div>
            <p class="text-xs mt-0.5" :class="isProblem(c) ? 'text-foreground' : 'text-muted-foreground'">{{ c.summary }}</p>
            <p v-if="c.used_by.length" class="text-xs text-muted-foreground mt-0.5">
              Needed by {{ c.used_by.join(', ') }}
            </p>

            <div v-if="c.status !== 'ok' && c.fix" class="mt-2 rounded-md bg-muted/50 px-3 py-2 space-y-2">
              <p class="text-xs">{{ c.fix.steps }}</p>
              <div v-if="c.fix.command" class="flex items-center gap-1 rounded bg-background border border-border px-2 py-1">
                <code class="num text-xs flex-1 min-w-0 overflow-x-auto whitespace-nowrap">{{ c.fix.command }}</code>
                <CopyButton :value="c.fix.command" label="Copy command" />
              </div>
              <div
                v-if="(local && (c.fix.settings_url || c.fix.can_request)) || c.fix.link"
                class="flex flex-wrap items-center gap-1.5"
              >
                <Button
                  v-if="local && c.fix.can_request"
                  size="sm"
                  class="h-7 gap-1.5 text-xs"
                  :disabled="asking !== null"
                  @click="ask(c)"
                >
                  <ShieldQuestion class="size-3.5" />
                  {{ asking === c.id ? 'Waiting for an answer…' : 'Ask macOS' }}
                </Button>
                <Button v-if="local && c.fix.settings_url" as-child variant="outline" size="sm" class="h-7 gap-1.5 text-xs">
                  <a :href="c.fix.settings_url"><Settings2 class="size-3.5" /> Open System Settings</a>
                </Button>
                <Button v-if="c.fix.link" as-child variant="outline" size="sm" class="h-7 gap-1.5 text-xs">
                  <a :href="c.fix.link.url" target="_blank" rel="noopener noreferrer">
                    <ExternalLink class="size-3.5" /> {{ c.fix.link.label }}
                  </a>
                </Button>
              </div>
            </div>
          </div>
          <span class="text-xs whitespace-nowrap" :class="status(c).class">{{ status(c).label }}</span>
        </li>
      </ul>
    </div>
  </section>
</template>
