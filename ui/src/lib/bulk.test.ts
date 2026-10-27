import { beforeEach, describe, expect, it, vi } from 'vitest'
import { BulkSkip, bulkToast, runBulk } from '@/lib/bulk'

const { toast, log } = vi.hoisted(() => ({
  toast: { success: vi.fn(), warning: vi.fn(), error: vi.fn() },
  log: vi.fn(),
}))
vi.mock('vue-sonner', () => ({ toast }))
vi.mock('@/stores/events', () => ({ useEventsStore: () => ({ log }) }))

beforeEach(() => vi.clearAllMocks())

describe('runBulk', () => {
  it('collects what worked and what failed, in order', async () => {
    const result = await runBulk([1, 2, 3, 4], async (n) => {
      if (n % 2 === 0) throw new Error(`no ${n}`)
    })
    expect(result).toEqual({
      ok: [1, 3],
      failed: [
        { item: 2, reason: 'no 2' },
        { item: 4, reason: 'no 4' },
      ],
    })
  })
  it('runs every item at once', async () => {
    const started: number[] = []
    let release!: () => void
    const gate = new Promise<void>((r) => (release = r))
    const run = runBulk([1, 2, 3], async (n) => {
      started.push(n)
      await gate
    })
    expect(started).toEqual([1, 2, 3])
    release()
    expect((await run).ok).toEqual([1, 2, 3])
  })
  it('falls back to "failed" for a non-Error', async () => {
    const result = await runBulk(['a'], () => Promise.reject('nope'))
    expect(result.failed).toEqual([{ item: 'a', reason: 'failed' }])
  })
  it('uses reasonOf, except for a BulkSkip', async () => {
    const result = await runBulk(
      ['a', 'b'],
      (s) => Promise.reject(s === 'a' ? new BulkSkip('still recording') : new Error('boom')),
      () => 'server said no',
    )
    expect(result.failed).toEqual([
      { item: 'a', reason: 'still recording' },
      { item: 'b', reason: 'server said no' },
    ])
  })
  it('handles no items', async () => {
    expect(await runBulk([], async () => {})).toEqual({ ok: [], failed: [] })
  })
})

describe('bulkToast', () => {
  const name = (n: number) => `clip ${n}`

  it('says how many when all worked', () => {
    bulkToast('Deleted', { ok: [1, 2], failed: [] }, name)
    expect(toast.success).toHaveBeenCalledWith('Deleted 2')
    expect(log).not.toHaveBeenCalled()
  })
  it('warns with the one shared reason when some failed', () => {
    bulkToast('Deleted', { ok: [1, 2, 3, 4, 5], failed: [{ item: 6, reason: 'still recording' }] }, name)
    const title = 'Deleted 5 · 1 failed: still recording'
    expect(toast.warning).toHaveBeenCalledWith(title, { description: 'clip 6: still recording' })
    expect(log).toHaveBeenCalledWith('warn', title, { detail: 'clip 6: still recording' })
  })
  it('errors when nothing worked, leaving mixed reasons to the description', () => {
    bulkToast('Deleted', { ok: [], failed: [{ item: 1, reason: 'a' }, { item: 2, reason: 'b' }] }, name)
    expect(toast.error).toHaveBeenCalledWith('Nothing deleted · 2 failed', { description: 'clip 1: a; clip 2: b' })
    expect(log).toHaveBeenCalledWith('error', 'Nothing deleted · 2 failed', { detail: 'clip 1: a\nclip 2: b' })
  })
  it('lists five failures in the toast and every one in the log', () => {
    const failed = [1, 2, 3, 4, 5, 6, 7].map((item) => ({ item, reason: 'x' }))
    bulkToast('Moved', { ok: [], failed }, name)
    expect(toast.error).toHaveBeenCalledWith('Nothing moved · 7 failed: x', {
      description: 'clip 1: x; clip 2: x; clip 3: x; clip 4: x; clip 5: x; and 2 more',
    })
    expect(log.mock.calls[0]![2].detail.split('\n')).toHaveLength(7)
  })
})
