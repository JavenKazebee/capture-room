import { defineStore } from 'pinia'
import { reactive, ref } from 'vue'
import { api, nodeApi } from '@/composables/useApi'
import { loadState, nodeLoadErrors } from '@/lib/loadState'
import { useNodesStore } from '@/stores/nodes'
import type { AccessTokenDto } from '@/types/generated/AccessTokenDto'
import type { AccessTokenRequest } from '@/types/generated/AccessTokenRequest'
import type { AccessTokenResultDto } from '@/types/generated/AccessTokenResultDto'
import type { ClockMode } from '@/types/generated/ClockMode'
import type { ControllerClockDto } from '@/types/generated/ControllerClockDto'
import type { MonitorSettingsDto } from '@/types/generated/MonitorSettingsDto'
import type { NodeSettingsDto } from '@/types/generated/NodeSettingsDto'
import type { UpdateNodeSettingsRequest } from '@/types/generated/UpdateNodeSettingsRequest'

/** Settings that span nodes: monitoring on each node, the controller's clock and the shared access token. */
export const useSettingsStore = defineStore('settings', () => {
  const nodes = useNodesStore()

  /** Each reachable node's monitoring settings, as last read. */
  const monitors = reactive(new Map<string, MonitorSettingsDto>())
  const monitorErrors = nodeLoadErrors()

  async function loadMonitors() {
    const ids = nodes.reachable.map((n) => n.id)
    for (const id of monitors.keys()) if (!ids.includes(id)) monitors.delete(id)
    monitorErrors.prune(ids)
    await Promise.all(
      ids.map(async (id) => {
        const s = await monitorErrors.track(id, () => nodeApi(id)<NodeSettingsDto>('/settings'))
        if (s) monitors.set(id, s.monitor)
      }),
    )
  }

  function monitorFor(nodeId: string) {
    return monitors.get(nodeId)
  }

  /** Set `monitor` on every reachable node; the nodes it failed on, each with its error. */
  async function applyMonitor(monitor: MonitorSettingsDto) {
    const targets = nodes.reachable
    const body = { name: null, monitor } satisfies UpdateNodeSettingsRequest
    const results = await Promise.allSettled(
      targets.map((n) => nodeApi(n.id)<NodeSettingsDto>('/settings', { method: 'PUT', body })),
    )
    const failed: { name: string; error: unknown }[] = []
    results.forEach((r, i) => {
      const node = targets[i]!
      if (r.status === 'rejected') failed.push({ name: node.name, error: r.reason })
      else {
        monitors.set(node.id, r.value.monitor)
        if (node.is_self && nodes.self) nodes.self.monitor = r.value.monitor
      }
    })
    return { applied: targets.length - failed.length, failed }
  }

  /** `null` until loaded, or while this instance isn't a controller. */
  const clock = ref<ControllerClockDto | null>(null)
  const clockLoad = loadState('clock settings')

  async function loadClock() {
    if (!nodes.isController) {
      clock.value = null
      return
    }
    const c = await clockLoad.track(() => api<ControllerClockDto>('/controller/clock'))
    if (c) clock.value = c
  }

  async function setClockMode(mode: ClockMode) {
    clock.value = await api<ControllerClockDto>('/controller/clock', { method: 'PUT', body: mode })
  }

  /** This node's token and pairing code; `null` until loaded. */
  const accessToken = ref<AccessTokenDto | null>(null)

  async function loadAccessToken() {
    accessToken.value = await api<AccessTokenDto>('/access-token')
  }

  /** Change the token here and on every paired node; the nodes that kept the old one. */
  async function setAccessToken(token: string) {
    const { failed } = await api<AccessTokenResultDto>('/access-token', {
      method: 'PUT',
      body: { token } satisfies AccessTokenRequest,
    })
    return failed
  }

  return {
    monitors,
    monitorErrors,
    loadMonitors,
    monitorFor,
    applyMonitor,
    clock,
    clockError: clockLoad.error,
    loadClock,
    setClockMode,
    accessToken,
    loadAccessToken,
    setAccessToken,
  }
})
