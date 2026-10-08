import type { TransportState } from '@/types/generated/TransportState'

/**
 * How each transport state reads. Tally red stays reserved for recording, so
 * playing is green and cued uses the primary color.
 */
export const TRANSPORT: Record<
  TransportState,
  { label: string; badge: string; dot: 'ok' | 'warn' | 'off' | 'info' }
> = {
  idle: { label: 'Idle', badge: 'bg-black/70 text-zinc-300', dot: 'off' },
  cued: { label: 'Cued', badge: 'bg-primary text-primary-foreground', dot: 'info' },
  playing: { label: 'Playing', badge: 'bg-success text-black', dot: 'ok' },
  paused: { label: 'Paused', badge: 'bg-warning text-black', dot: 'warn' },
  ended: { label: 'Ended', badge: 'bg-black/70 text-zinc-300', dot: 'off' },
}

/** Drag data carrying a media library entry's id (into a playlist). */
export const MEDIA_DRAG = 'application/x-cr-media'
