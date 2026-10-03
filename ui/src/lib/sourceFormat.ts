import type { SourceCapabilitiesDto } from '@/types/generated/SourceCapabilitiesDto'

/** `30 fps`, `29.97 fps`; `short` drops the unit (`30`, `29.97`). */
export function fpsLabel([n, d]: [number, number], short = false) {
  const v = d === 1 ? String(n) : (n / d).toFixed(2).replace(/\.?0+$/, '')
  return short ? v : `${v} fps`
}

export function resolutionLabel(c: SourceCapabilitiesDto) {
  return `${c.max_width}×${c.max_height}`
}
