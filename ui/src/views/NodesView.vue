<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { errorMessage, nodeApi } from '@/composables/useApi'
import { useNodesStore } from '@/stores/nodes'
import type { MonitorSettingsDto } from '@/types/generated/MonitorSettingsDto'
import type { NodeSettingsDto } from '@/types/generated/NodeSettingsDto'
import type { StorageVolumeDto } from '@/types/generated/StorageVolumeDto'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import OptionSelect from '@/components/OptionSelect.vue'

const store = useNodesStore()

const toggling = ref(false)
const addNodeUrl = ref('')
const addNodeError = ref('')
const addNodeLoading = ref(false)

/** Storage volumes per node id; `null` = failed to load. */
const storage = reactive(new Map<string, StorageVolumeDto[] | null>())

async function loadStorage(nodeId: string) {
  storage.set(
    nodeId,
    await nodeApi(nodeId)<StorageVolumeDto[]>('/storage').catch(() => null),
  )
}

async function load() {
  await store.load()
  await Promise.all(store.reachable.map((n) => loadStorage(n.id)))
}

async function toggleController() {
  if (toggling.value) return
  toggling.value = true
  try {
    await store.setController(!store.isController)
    await Promise.all(store.reachable.map((n) => loadStorage(n.id)))
  } finally {
    toggling.value = false
  }
}

async function addNode() {
  if (!addNodeUrl.value.trim() || addNodeLoading.value) return
  addNodeError.value = ''
  addNodeLoading.value = true
  try {
    await store.add(addNodeUrl.value.trim())
    addNodeUrl.value = ''
    await Promise.all(store.reachable.map((n) => loadStorage(n.id)))
  } catch (e) {
    addNodeError.value = errorMessage(e, 'Failed to add node')
  } finally {
    addNodeLoading.value = false
  }
}

async function removeNode(id: string) {
  await store.remove(id)
  storage.delete(id)
}

// ── Monitor settings (applied to every reachable node) ───────────────────────

/** The form's draft: this node's settings once loaded, sent to every node on Apply. */
const monitor = ref<MonitorSettingsDto | null>(null)
const monitorSaving = ref(false)
const monitorError = ref('')

const THUMB_SIZES = [
  { label: '320×180', value: '320x180' },
  { label: '640×360', value: '640x360' },
  { label: '1280×720', value: '1280x720' },
]
const THUMB_FPS = [1, 2, 5, 10].map((v) => ({ label: `${v} fps`, value: v }))
const LEVEL_INTERVALS = [50, 100, 200, 500].map((v) => ({ label: `${v} ms`, value: v }))

const thumbSize = computed({
  get: () => (monitor.value ? `${monitor.value.thumb_width}x${monitor.value.thumb_height}` : ''),
  set: (v: string) => {
    const [w, h] = v.split('x').map(Number)
    if (monitor.value) monitor.value = { ...monitor.value, thumb_width: w!, thumb_height: h! }
  },
})

async function saveMonitorSettings() {
  if (!monitor.value || monitorSaving.value) return
  monitorSaving.value = true
  monitorError.value = ''
  try {
    const body = { monitor: monitor.value }
    const results = await Promise.allSettled(
      store.reachable.map((n) =>
        nodeApi(n.id)<NodeSettingsDto>('/settings', { method: 'PUT', body }),
      ),
    )
    const failed = results.filter((r) => r.status === 'rejected').length
    if (failed) monitorError.value = `Couldn't apply to ${failed} node${failed > 1 ? 's' : ''}.`
    const first = results.find((r) => r.status === 'fulfilled')
    if (first) {
      monitor.value = first.value.monitor
      if (store.self) store.self.monitor = first.value.monitor
    }
  } finally {
    monitorSaving.value = false
  }
}

function formatUptime(secs: number): string {
  const h = Math.floor(secs / 3600)
  const m = Math.floor((secs % 3600) / 60)
  if (h > 0) return `${h}h ${m}m`
  return `${m}m`
}

function formatBytes(n: number): string {
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let i = 0
  while (n >= 1000 && i < units.length - 1) {
    n /= 1000
    i++
  }
  return `${n.toFixed(n >= 100 || i === 0 ? 0 : 1)} ${units[i]}`
}

function usedPct(v: StorageVolumeDto) {
  return v.total_bytes ? Math.round(((v.total_bytes - v.available_bytes) / v.total_bytes) * 100) : 0
}

onMounted(async () => {
  await load()
  if (store.self) monitor.value = { ...store.self.monitor }
})
</script>

<template>
  <div class="p-6 max-w-3xl">
    <h1 class="text-2xl font-semibold mb-6">Nodes</h1>

    <!-- This machine -->
    <section class="rounded-lg border border-border bg-card p-4 mb-6">
      <div class="flex items-start justify-between gap-4">
        <div>
          <h2 class="text-sm font-semibold mb-1">This machine</h2>
          <p class="text-xs text-muted-foreground">
            {{ store.self?.node_name }} ·
            <span class="font-mono">{{ store.self?.node_id?.slice(0, 8) }}</span>
          </p>
        </div>
        <Button
          :variant="store.isController ? 'default' : 'outline'"
          size="default"
          :disabled="toggling || !store.self"
          @click="toggleController"
        >
          {{ store.isController ? 'Controller: on' : 'Controller: off' }}
        </Button>
      </div>
      <p class="mt-3 text-xs text-muted-foreground">
        Every machine is a node: it can see and record its own sources and storage.
        Turning on the controller also lets this machine find other nodes on the network
        and control them from here. The change applies immediately.
      </p>
    </section>

    <!-- Monitoring -->
    <section v-if="monitor" class="rounded-lg border border-border bg-card p-4 mb-6">
      <h2 class="text-sm font-semibold mb-3">Monitoring</h2>
      <div class="flex items-center gap-2 flex-wrap">
        <span class="text-xs text-muted-foreground">Thumbnail</span>
        <OptionSelect v-model="thumbSize" :options="THUMB_SIZES" :disabled="monitorSaving" class="w-28" />
        <OptionSelect v-model="monitor.thumb_fps" :options="THUMB_FPS" :disabled="monitorSaving" class="w-20" />
        <span class="text-xs text-muted-foreground ml-2">Audio meter</span>
        <OptionSelect
          v-model="monitor.level_interval_ms"
          :options="LEVEL_INTERVALS"
          :disabled="monitorSaving"
          class="w-20"
        />
        <Button size="sm" variant="outline" :disabled="monitorSaving" @click="saveMonitorSettings">
          {{ monitorSaving ? 'Applying…' : 'Apply' }}
        </Button>
      </div>
      <p v-if="monitorError" class="text-xs text-destructive mt-2">{{ monitorError }}</p>
      <p class="mt-3 text-xs text-muted-foreground">
        How often each source's thumbnail and audio meter update on the dashboard. Applied
        to every reachable node.
      </p>
    </section>

    <!-- Nodes -->
    <section>
      <h2 class="text-sm font-semibold mb-3">
        {{ store.isController ? 'Controlled nodes' : 'Node' }}
      </h2>

      <div v-if="store.isController" class="mb-4">
        <div class="flex gap-2">
          <Input
            v-model="addNodeUrl"
            placeholder="192.168.1.x:7700"
            class="font-mono text-xs"
            @keydown.enter="addNode"
          />
          <Button size="sm" :disabled="addNodeLoading || !addNodeUrl.trim()" @click="addNode">
            {{ addNodeLoading ? 'Adding…' : 'Add node' }}
          </Button>
        </div>
        <p v-if="addNodeError" class="text-xs text-destructive mt-2">{{ addNodeError }}</p>
        <p class="text-xs text-muted-foreground mt-2">
          Nodes on the same network are found automatically. Add one by address if it's on
          another subnet.
        </p>
      </div>

      <div class="rounded-lg border border-border bg-card divide-y divide-border">
        <div v-for="node in store.nodes" :key="node.id" class="px-4 py-3">
          <div class="flex items-center gap-3">
            <span
              class="w-2 h-2 rounded-full shrink-0"
              :class="node.healthy ? 'bg-green-500' : 'bg-red-500'"
            />
            <div class="flex-1 min-w-0">
              <div class="flex items-center gap-2">
                <span class="text-sm font-medium truncate">{{ node.name }}</span>
                <Badge v-if="node.is_self" variant="outline">this machine</Badge>
                <Badge v-else-if="node.manual" variant="secondary">added by address</Badge>
              </div>
              <div class="text-xs text-muted-foreground truncate">
                {{ node.url || 'local' }}
                <template v-if="node.healthy">
                  · v{{ node.version }} · up {{ formatUptime(node.uptime_secs) }}
                </template>
                <template v-else> · unreachable</template>
              </div>
            </div>
            <Button
              v-if="!node.is_self && store.isController"
              variant="outline"
              size="sm"
              @click="removeNode(node.id)"
            >
              Remove
            </Button>
          </div>

          <!-- Storage -->
          <div v-if="node.healthy" class="mt-3 pl-5 space-y-2">
            <p v-if="storage.get(node.id) === null" class="text-xs text-destructive">
              Couldn't read storage.
            </p>
            <div
              v-for="vol in storage.get(node.id) ?? []"
              :key="vol.mount_point"
              class="text-xs"
            >
              <div class="flex justify-between gap-2 mb-1">
                <span class="font-mono truncate" :title="vol.mount_point">
                  {{ vol.mount_point }}
                  <span class="text-muted-foreground font-sans">
                    {{ vol.name && vol.name !== vol.mount_point ? `· ${vol.name}` : '' }}
                    {{ vol.removable ? '· removable' : '' }}
                  </span>
                </span>
                <span class="text-muted-foreground shrink-0">
                  {{ formatBytes(vol.available_bytes) }} free of {{ formatBytes(vol.total_bytes) }}
                </span>
              </div>
              <div class="h-1.5 rounded-full bg-muted overflow-hidden">
                <div
                  class="h-full rounded-full"
                  :class="usedPct(vol) > 90 ? 'bg-destructive' : 'bg-primary'"
                  :style="{ width: `${usedPct(vol)}%` }"
                />
              </div>
            </div>
          </div>
        </div>
      </div>
    </section>
  </div>
</template>
