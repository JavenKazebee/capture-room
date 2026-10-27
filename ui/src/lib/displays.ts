import type { DisplayDto } from '@/types/generated/DisplayDto'
import type { VideoFormat } from '@/types/generated/VideoFormat'

/** A refresh rate in mHz as people read it: 59940 → "59.94 Hz". */
export function hz(mhz: number) {
  return `${Number((mhz / 1000).toFixed(3))} Hz`
}

/** The mode a screen will be set to for `format`: its own rate, then double, so nothing judders. Null if it has none. */
export function matchingMode(d: DisplayDto, format: VideoFormat) {
  const { width, height, fps_num, fps_den } = format
  const mhz = Math.floor((fps_num * 1000) / Math.max(fps_den, 1))
  for (const rate of [mhz, mhz * 2]) {
    const m = d.modes.find((m) => m.width === width && m.height === height && Math.abs(m.refresh_mhz - rate) <= 10)
    if (m) return m
  }
  return null
}
