import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api, sourceKey } from '@/composables/useApi'
import { loadState } from '@/lib/loadState'
import { notifyError } from '@/lib/notify'
import { onReconnect } from '@/lib/reconnect'
import { moveKey } from '@/lib/layoutOrder'
import type { RecordLayoutDto } from '@/types/generated/RecordLayoutDto'
import type { SourceRef } from '@/types/generated/SourceRef'

function refOf(key: string): SourceRef {
  const i = key.indexOf('/')
  return { node_id: key.slice(0, i), source_id: key.slice(i + 1) }
}

/** The Record page's layout, kept on the controller: the sources' order and their tags, by source key. */
export const useLayoutStore = defineStore('layout', () => {
  /** Keys placed by hand, first to last; may name sources this UI can't see right now. */
  const order = ref<string[]>([])
  const tagsByKey = ref(new Map<string, string[]>())
  const { loaded, error, track } = loadState('layout')

  function apply(l: RecordLayoutDto) {
    order.value = l.order.map((s) => sourceKey(s.node_id, s.source_id))
    tagsByKey.value = new Map(l.tags.map((t) => [sourceKey(t.node_id, t.source_id), t.tags]))
  }

  async function load() {
    await track(async () => apply(await api<RecordLayoutDto>('/record-layout')))
  }

  onReconnect(load)

  const position = computed(() => new Map(order.value.map((k, i) => [k, i])))

  /** `list` in layout order; sources never placed keep their order, after the placed ones. */
  function sorted<T extends { key: string }>(list: readonly T[]): T[] {
    const pos = position.value
    return list
      .map((s, i) => ({ s, i, p: pos.get(s.key) ?? Infinity }))
      .sort((a, b) => a.p - b.p || a.i - b.i)
      .map((x) => x.s)
  }

  function tagsOf(key: string): string[] {
    return tagsByKey.value.get(key) ?? []
  }

  /** Every tag in use, by name. */
  const allTags = computed(() => {
    const byLower = new Map<string, string>()
    for (const tags of tagsByKey.value.values()) for (const t of tags) byLower.set(t.toLowerCase(), t)
    return [...byLower.values()].sort((a, b) => a.localeCompare(b))
  })

  function hasTag(key: string, tag: string) {
    const t = tag.toLowerCase()
    return tagsOf(key).some((x) => x.toLowerCase() === t)
  }

  /**
   * Move `key` to `target`'s place among `known` (every source this UI shows, in layout order).
   * Places the UI can't see stay where they were.
   */
  async function move(key: string, target: string, known: readonly string[]) {
    const next = moveKey([...order.value, ...known.filter((k) => !position.value.has(k))], key, target)
    if (!next) return
    const prev = order.value
    order.value = next
    try {
      apply(await api<RecordLayoutDto>('/record-layout/order', { method: 'PUT', body: { order: next.map(refOf) } }))
    } catch (e) {
      order.value = prev
      notifyError('Couldn’t save the order', e)
    }
  }

  async function changeTags(keys: readonly string[], add: string[], remove: string[] = []) {
    if (!keys.length || (!add.length && !remove.length)) return
    try {
      apply(
        await api<RecordLayoutDto>('/record-layout/tags', {
          method: 'POST',
          body: { sources: keys.map(refOf), add, remove },
        }),
      )
    } catch (e) {
      notifyError('Couldn’t change the tags', e)
    }
  }

  return { order, loaded, error, load, sorted, tagsOf, allTags, hasTag, move, changeTags }
})
