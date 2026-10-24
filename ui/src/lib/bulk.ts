import { toast } from 'vue-sonner'
import { errorMessage } from '@/composables/useApi'
import { useEventsStore } from '@/stores/events'

/** An item a bulk action couldn't do, and why (short, e.g. "still recording"). */
export interface BulkFailure<T> {
  item: T
  reason: string
}

export interface BulkResult<T> {
  ok: T[]
  failed: BulkFailure<T>[]
}

/** Thrown (or rejected with) to skip an item with a reason, without calling the server. */
export class BulkSkip extends Error {}

/**
 * Runs `action` on every item at once and collects what worked and what didn't,
 * so one failure never stops the rest. `reasonOf` turns an error into a short reason.
 */
export async function runBulk<T>(
  items: T[],
  action: (item: T) => Promise<unknown>,
  reasonOf: (e: unknown) => string = (e) => errorMessage(e, 'failed'),
): Promise<BulkResult<T>> {
  const settled = await Promise.allSettled(items.map((item) => action(item)))
  const result: BulkResult<T> = { ok: [], failed: [] }
  settled.forEach((r, i) => {
    const item = items[i]!
    if (r.status === 'fulfilled') result.ok.push(item)
    else result.failed.push({ item, reason: r.reason instanceof BulkSkip ? r.reason.message : reasonOf(r.reason) })
  })
  return result
}

/**
 * One toast for a bulk action, e.g. "Deleted 5 · 1 failed: still recording".
 * Failures are listed by `nameOf` and kept in the event log.
 */
export function bulkToast<T>(done: string, result: BulkResult<T>, nameOf: (item: T) => string) {
  const { ok, failed } = result
  if (!failed.length) {
    toast.success(`${done} ${ok.length}`)
    return
  }
  const reasons = [...new Set(failed.map((f) => f.reason))]
  const title = `${ok.length ? `${done} ${ok.length}` : `Nothing ${done.toLowerCase()}`} · ${failed.length} failed${reasons.length === 1 ? `: ${reasons[0]}` : ''}`
  const lines = failed.map((f) => `${nameOf(f.item)}: ${f.reason}`)
  const description = (lines.length > 5 ? [...lines.slice(0, 5), `and ${lines.length - 5} more`] : lines).join('; ')
  if (ok.length) toast.warning(title, { description })
  else toast.error(title, { description })
  useEventsStore().log(ok.length ? 'warn' : 'error', title, { detail: lines.join('\n') })
}
