import { defineStore } from 'pinia'
import { ref, shallowReactive } from 'vue'
import { toast } from 'vue-sonner'
import { plural } from '@/lib/format'
import { notifyError } from '@/lib/notify'
import { nodeApi, sourceKey } from '@/composables/useApi'
import { useNodesStore } from '@/stores/nodes'
import type { ChannelLevelDto } from '@/types/generated/ChannelLevelDto'
import type { ConfiguredSourceDto } from '@/types/generated/ConfiguredSourceDto'
import type { ConfiguredSourceRequest } from '@/types/generated/ConfiguredSourceRequest'
import type { DecklinkProfile } from '@/types/generated/DecklinkProfile'
import type { DecklinkStatusDto } from '@/types/generated/DecklinkStatusDto'
import type { DeviceDto } from '@/types/generated/DeviceDto'
import type { DisplayDto } from '@/types/generated/DisplayDto'
import type { DirListingDto } from '@/types/generated/DirListingDto'
import type { LinkState } from '@/types/generated/LinkState'
import type { SourceDto } from '@/types/generated/SourceDto'
import type { SourceType } from '@/types/generated/SourceType'

/** A node's source tagged with the node it lives on (`id` is only unique per node). */
export type Source = SourceDto & {
  node_id: string
  /** `${node_id}/${id}` — unique across nodes. */
  key: string
}

// Audio levels updated ~10fps — shallow to avoid deep reactivity overhead.
// Both maps are keyed by sourceKey(node_id, source_id).
export const audioLevels = shallowReactive(new Map<string, ChannelLevelDto[]>())

// Thumbnail cache-bust counter incremented on each thumbnail.updated event
export const thumbnailSeqs = shallowReactive(new Map<string, number>())

/** Source types a node is configured with, rather than discovering them. */
export type ConfiguredKind = Exclude<SourceType, 'ndi'>
export const CONFIGURED_TYPES: readonly SourceType[] = ['test', 'file', 'stream', 'device', 'decklink', 'whip', 'channel']

export const useSourcesStore = defineStore('sources', () => {
  const nodes = useNodesStore()
  const sources = ref<Source[]>([])

  function tag(nodeId: string, list: SourceDto[]): Source[] {
    return list.map((s) => ({ ...s, node_id: nodeId, key: sourceKey(nodeId, s.id) }))
  }

  /** Apply a `feed.status` event: the current timecode, monitor error and link state. */
  function updateStatus(
    nodeId: string,
    sourceId: string,
    tc: string | null,
    error: string | null,
    link: LinkState | null,
  ) {
    const key = sourceKey(nodeId, sourceId)
    const s = sources.value.find((s) => s.key === key)
    if (!s) return
    s.timecode = tc
    s.error = error
    s.link = link
  }

  /** Replace one node's sources, keeping the others. */
  function setForNode(nodeId: string, list: SourceDto[]) {
    sources.value = [...sources.value.filter((s) => s.node_id !== nodeId), ...tag(nodeId, list)]
  }

  /** Whether every reachable node's sources have been fetched once. */
  const loaded = ref(false)
  let pending: Promise<void> | null = null

  function loadSources() {
    const load = (async () => {
      if (nodes.nodes.length === 0) await nodes.load()
      const results = await Promise.all(
        nodes.reachable.map(async (n) =>
          tag(n.id, await nodeApi(n.id)<SourceDto[]>('/sources').catch(() => [])),
        ),
      )
      sources.value = results.flat()
      loaded.value = true
    })()
    pending = load
    load.finally(() => pending === load && (pending = null)).catch(() => {})
    return load
  }

  /** Load the sources unless they're loaded or loading (App loads them at start). */
  function ensureLoaded() {
    if (loaded.value) return Promise.resolve()
    return pending ?? loadSources()
  }

  async function refreshNode(nodeId: string) {
    setForNode(nodeId, await nodeApi(nodeId)<SourceDto[]>('/sources'))
  }

  async function scan(nodeId: string) {
    setForNode(nodeId, await nodeApi(nodeId)<SourceDto[]>('/sources/scan', { method: 'POST' }))
  }

  async function scanAll() {
    await Promise.all(nodes.reachable.map((n) => scan(n.id)))
  }

  /** Scan every reachable node, toasting how it went. */
  async function scanWithToast() {
    const before = new Set(sources.value.map((s) => s.key))
    const id = toast.loading('Scanning for sources…', { id: 'scan' })
    try {
      await scanAll()
      const added = sources.value.filter((s) => !before.has(s.key)).length
      toast.success(`Scan complete — ${plural(sources.value.length, 'source')}`, {
        id,
        description: added ? `${added} new` : 'No new sources found',
      })
    } catch (e) {
      toast.dismiss(id)
      notifyError('Scan failed', e)
    }
  }

  async function configs(nodeId: string) {
    return nodeApi(nodeId)<ConfiguredSourceDto[]>('/configured-sources')
  }

  /** One configured source's stored config; null if the node no longer has it. */
  async function getConfig(nodeId: string, id: string) {
    return (await configs(nodeId)).find((c) => c.id === id) ?? null
  }

  async function createSource(nodeId: string, input: ConfiguredSourceRequest) {
    const created = await nodeApi(nodeId)<ConfiguredSourceDto>('/configured-sources', {
      method: 'POST',
      body: input,
    })
    await refreshNode(nodeId)
    return created
  }

  async function updateSource(nodeId: string, id: string, input: ConfiguredSourceRequest) {
    const updated = await nodeApi(nodeId)<ConfiguredSourceDto>(`/configured-sources/${id}`, {
      method: 'PUT',
      body: input,
    })
    await refreshNode(nodeId)
    return updated
  }

  async function deleteSource(nodeId: string, id: string) {
    await nodeApi(nodeId)(`/configured-sources/${id}`, { method: 'DELETE' })
    await refreshNode(nodeId)
  }

  /**
   * The other nodes connected to the same NDI sender as `s`: an NDI source's
   * id comes from the sender's name, so it's the same on every node.
   */
  function alsoOn(s: Source): string[] {
    if (s.source_type !== 'ndi') return []
    return sources.value.filter((o) => o.id === s.id && o.node_id !== s.node_id).map((o) => o.node_id)
  }

  /** A directory on the node (its home directory if `path` is omitted). */
  async function listFiles(nodeId: string, path?: string) {
    return nodeApi(nodeId)<DirListingDto>('/files', { query: path ? { path } : {} })
  }

  /** Capture devices and audio outputs on the node. */
  async function listDevices(nodeId: string) {
    return nodeApi(nodeId)<DeviceDto[]>('/devices')
  }

  /** The node's Decklink cards and what uses their sub-devices. */
  async function listDecklink(nodeId: string) {
    return nodeApi(nodeId)<DecklinkStatusDto>('/decklink')
  }

  /** Set how a Decklink card splits its connectors; what uses the card restarts. */
  async function setDecklinkProfile(nodeId: string, card: string, profile: DecklinkProfile) {
    return nodeApi(nodeId)<DecklinkStatusDto>(`/decklink/${encodeURIComponent(card)}/profile`, {
      method: 'PUT',
      body: { profile },
    })
  }

  /** Screens attached to the node, for display outputs. */
  async function listDisplays(nodeId: string) {
    return nodeApi(nodeId)<DisplayDto[]>('/displays')
  }

  return {
    sources,
    loaded,
    ensureLoaded,
    listDevices,
    listDisplays,
    listDecklink,
    setDecklinkProfile,
    updateStatus,
    loadSources,
    refreshNode,
    scan,
    scanAll,
    scanWithToast,
    configs,
    getConfig,
    createSource,
    updateSource,
    deleteSource,
    listFiles,
    alsoOn,
  }
})
