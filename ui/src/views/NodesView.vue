<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useApi } from '@/composables/useApi'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'

interface NodeDto {
  id: string
  name: string
  url: string
  version: string
  healthy: boolean
  uptime_secs: number
  is_self: boolean
}

interface Settings {
  node_id: string
  node_name: string
  role: 'node' | 'aggregator'
  persisted_role: 'node' | 'aggregator' | null
}

const { api } = useApi()

const settings = ref<Settings | null>(null)
const nodes = ref<NodeDto[]>([])
const saving = ref(false)
const restartRequired = ref(false)
const addNodeUrl = ref('')
const addNodeError = ref('')
const addNodeLoading = ref(false)

const isAggregator = computed(() => settings.value?.role === 'aggregator')

async function load() {
  const [s, n] = await Promise.all([
    api<Settings>('/settings').catch(() => null),
    api<NodeDto[]>('/nodes').catch(() => []),
  ])
  settings.value = s
  nodes.value = n
  // If the persisted role differs from the effective one, a restart is pending.
  if (s && s.persisted_role && s.persisted_role !== s.role) {
    restartRequired.value = true
  }
}

async function setRole(role: 'node' | 'aggregator') {
  if (saving.value || !settings.value) return
  saving.value = true
  try {
    const res = await api<{ persisted_role: string; restart_required: boolean }>('/settings', {
      method: 'PUT',
      body: { role },
    })
    settings.value.persisted_role = res.persisted_role as Settings['persisted_role']
    restartRequired.value = res.restart_required
  } finally {
    saving.value = false
  }
}

function formatUptime(secs: number): string {
  const h = Math.floor(secs / 3600)
  const m = Math.floor((secs % 3600) / 60)
  if (h > 0) return `${h}h ${m}m`
  return `${m}m`
}

async function addNode() {
  if (!addNodeUrl.value.trim() || addNodeLoading.value) return
  addNodeError.value = ''
  addNodeLoading.value = true
  try {
    await api('/nodes', { method: 'POST', body: { url: addNodeUrl.value.trim() } })
    addNodeUrl.value = ''
    await load()
  } catch (e: any) {
    addNodeError.value = e?.data ?? e?.message ?? 'Failed to add node'
  } finally {
    addNodeLoading.value = false
  }
}

onMounted(load)
</script>

<template>
  <div class="p-6 max-w-3xl">
    <h1 class="text-2xl font-semibold mb-6">Nodes</h1>

    <!-- Control station setting -->
    <section class="rounded-lg border border-border bg-card p-4 mb-6">
      <h2 class="text-sm font-semibold mb-1">This machine</h2>
      <p class="text-xs text-muted-foreground mb-4">
        {{ settings?.node_name }} · <span class="font-mono">{{ settings?.node_id?.slice(0, 8) }}</span>
      </p>

      <div class="flex items-center gap-2 mb-3">
        <span class="text-xs text-muted-foreground w-28">Current role</span>
        <Badge :variant="isAggregator ? 'default' : 'secondary'">
          {{ isAggregator ? 'Control station' : 'Capture node' }}
        </Badge>
      </div>

      <div class="flex items-center gap-2">
        <span class="text-xs text-muted-foreground w-28">Set role</span>
        <Button
          :variant="settings?.persisted_role === 'node' || (!settings?.persisted_role && settings?.role === 'node') ? 'default' : 'outline'"
          size="default"
          :disabled="saving"
          @click="setRole('node')"
        >
          Capture node
        </Button>
        <Button
          :variant="settings?.persisted_role === 'aggregator' || (!settings?.persisted_role && settings?.role === 'aggregator') ? 'default' : 'outline'"
          size="default"
          :disabled="saving"
          @click="setRole('aggregator')"
        >
          Control station
        </Button>
      </div>

      <p
        v-if="restartRequired"
        class="mt-3 text-xs text-yellow-600 dark:text-yellow-400"
      >
        Restart required for this change to take effect.
      </p>
      <p class="mt-3 text-xs text-muted-foreground">
        The control station discovers other machines on the network and shows all their
        feeds in one place. Capture nodes record locally and are aggregated by the control
        station.
      </p>
    </section>

    <!-- Discovered nodes -->
    <section v-if="isAggregator">
      <div class="flex items-center justify-between mb-3">
        <h2 class="text-sm font-semibold">Discovered nodes</h2>
      </div>
      <!-- Manual add -->
      <div class="flex gap-2 mb-4">
        <Input
          v-model="addNodeUrl"
          placeholder="http://192.168.1.x:7700"
          class="font-mono text-xs"
          @keydown.enter="addNode"
        />
        <Button size="sm" :disabled="addNodeLoading || !addNodeUrl.trim()" @click="addNode">
          {{ addNodeLoading ? 'Adding…' : 'Add node' }}
        </Button>
      </div>
      <p v-if="addNodeError" class="text-xs text-red-500 mb-3">{{ addNodeError }}</p>
      <div class="rounded-lg border border-border bg-card divide-y divide-border">
        <div
          v-for="node in nodes"
          :key="node.id"
          class="flex items-center gap-3 px-4 py-3"
        >
          <span
            class="w-2 h-2 rounded-full shrink-0"
            :class="node.healthy ? 'bg-green-500' : 'bg-red-500'"
          />
          <div class="flex-1 min-w-0">
            <div class="flex items-center gap-2">
              <span class="text-sm font-medium truncate">{{ node.name }}</span>
              <Badge v-if="node.is_self" variant="outline">this machine</Badge>
            </div>
            <div class="text-xs text-muted-foreground truncate">
              {{ node.url || 'local' }} · v{{ node.version }} · up {{ formatUptime(node.uptime_secs) }}
            </div>
          </div>
        </div>
      </div>
    </section>
  </div>
</template>
