import type { LinkState } from '@/types/generated/LinkState'

/**
 * How a live source's link state reads. Amber (`warn`) means frames should
 * be coming and aren't; waiting for a sender to connect is neutral.
 */
export const LINK: Record<LinkState, { label: string; short: string; dot: 'ok' | 'warn' | 'off'; message: string }> = {
  live: { label: 'Live', short: 'Live', dot: 'ok', message: '' },
  connecting: { label: 'Connecting…', short: 'Connecting', dot: 'warn', message: 'Connecting…' },
  waiting: { label: 'Waiting for sender', short: 'Waiting', dot: 'off', message: 'Waiting for a sender' },
  reconnecting: { label: 'Reconnecting…', short: 'Reconnecting', dot: 'warn', message: 'No signal — reconnecting' },
}
