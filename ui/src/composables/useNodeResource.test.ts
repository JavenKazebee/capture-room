import { describe, expect, it } from 'vitest'
import { nextTick, ref } from 'vue'
import { useNodeResource } from '@/composables/useNodeResource'

/** A fetch whose answers the test releases by hand, per node. */
function controlledFetch() {
  const waiting = new Map<string, { resolve: (v: string[]) => void; reject: (e: unknown) => void }>()
  let calls = 0
  const fetch = (node: string) => {
    calls++
    return new Promise<string[]>((resolve, reject) => waiting.set(node, { resolve, reject }))
  }
  return { fetch, waiting, calls: () => calls }
}

const settle = () => new Promise((r) => setTimeout(r))

describe('useNodeResource', () => {
  it('loads the node it starts with', async () => {
    const f = controlledFetch()
    const r = useNodeResource(ref('a'), f.fetch, () => [], 'failed')
    expect(r.loading.value).toBe(true)
    f.waiting.get('a')!.resolve(['x'])
    await settle()
    expect(r.data.value).toEqual(['x'])
    expect(r.loading.value).toBe(false)
  })

  it('drops an answer for a node no longer picked', async () => {
    const f = controlledFetch()
    const node = ref('a')
    const r = useNodeResource(node, f.fetch, () => [], 'failed')
    node.value = 'b'
    await nextTick()
    f.waiting.get('b')!.resolve(['from b'])
    f.waiting.get('a')!.resolve(['from a'])
    await settle()
    expect(r.data.value).toEqual(['from b'])
  })

  it('shares a refresh for the same node', async () => {
    const f = controlledFetch()
    const r = useNodeResource(ref('a'), f.fetch, () => [], 'failed')
    const again = r.refresh()
    expect(f.calls()).toBe(1)
    f.waiting.get('a')!.resolve(['x'])
    await again
    expect(r.data.value).toEqual(['x'])
  })

  it('empties and explains on failure', async () => {
    const f = controlledFetch()
    const r = useNodeResource(ref('a'), f.fetch, () => ['empty'], 'Could not list.')
    f.waiting.get('a')!.reject(new Error('boom'))
    await settle()
    expect(r.data.value).toEqual(['empty'])
    expect(r.error.value).toBeTruthy()
  })
})
