import type { ClockSourceDto } from '@/types/generated/ClockSourceDto'
import type { ClockStatusDto } from '@/types/generated/ClockStatusDto'

export interface ClockSummary {
  status: 'ok' | 'warn' | 'error' | 'off'
  /** What the node runs on, e.g. "Studio A's clock". */
  label: string
  /** How well it's doing, e.g. "±0.2 ms" or "syncing…". */
  detail: string
  /** A longer explanation for a tooltip. */
  title: string
}

export function clockSourceLabel(source: ClockSourceDto): string {
  switch (source.kind) {
    case 'local':
      return 'Own clock'
    case 'controller':
      return `${source.controller_name}'s clock`
    case 'ptp':
      return `PTP domain ${source.domain}`
  }
}

/** The network delay as an accuracy hint: sync is good to a fraction of it. */
function delayLabel(us: number): string {
  return us < 1000 ? `${Math.max(1, Math.round(us))} µs delay` : `${(us / 1000).toFixed(1)} ms delay`
}

/** One line for a node's clock. `null` for nodes that predate clock sync. */
export function clockSummary(clock: ClockStatusDto | null): ClockSummary {
  if (!clock) {
    return { status: 'off', label: 'Clock', detail: 'not reported', title: 'This node is too old to report its clock.' }
  }
  const label = clockSourceLabel(clock.source)
  const stale = clock.stale_sources
    ? ` ${clock.stale_sources} source${clock.stale_sources === 1 ? '' : 's'} still on the previous clock until idle.`
    : ''
  if (clock.error) {
    return { status: 'error', label, detail: 'error', title: clock.error }
  }
  if (clock.pending) {
    return {
      status: 'warn',
      label,
      detail: `switching to ${clockSourceLabel(clock.pending)}…`,
      title: `Waiting for ${clockSourceLabel(clock.pending)} to sync; ${label.toLowerCase()} is used until then.${stale}`,
    }
  }
  if (clock.source.kind === 'local') {
    return { status: 'off', label, detail: '', title: `Runs on this machine's own clock.${stale}` }
  }
  if (clock.lost) {
    return {
      status: 'warn',
      label,
      detail: 'lost',
      title: 'No word from the clock master lately: running on its last known rate and drifting slowly.',
    }
  }
  if (!clock.synced) {
    return { status: 'warn', label, detail: 'syncing…', title: `Not in sync yet.${stale}` }
  }
  return {
    status: clock.stale_sources ? 'warn' : 'ok',
    label,
    detail: clock.delay_us != null ? delayLabel(clock.delay_us) : 'in sync',
    title: `In sync with ${label.toLowerCase()}.${stale}`,
  }
}
