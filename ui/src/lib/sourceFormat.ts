import type { MediaInfo } from '@/types/generated/MediaInfo'
import type { SourceCapabilitiesDto } from '@/types/generated/SourceCapabilitiesDto'
import { formatFramerate } from '@/lib/codecs'

/** `30 fps`, `29.97 fps`; `short` drops the unit (`30`, `29.97`). A 0 rate is variable. */
export function fpsLabel([n, d]: [number, number], short = false) {
  if (n === 0) return short ? 'VFR' : 'variable fps'
  const v = formatFramerate([n, d])
  return short ? v : `${v} fps`
}

/** `1080p30`, `2160p59.94`; other sizes as `1440×1080 30`. */
export function formatLabel(m: Pick<MediaInfo, 'width' | 'height' | 'fps_num' | 'fps_den'>) {
  const rate = fpsLabel([m.fps_num, m.fps_den], true)
  const standard = [360, 480, 576, 720, 1080, 1440, 2160, 4320].includes(m.height) && m.width >= m.height
  return standard ? `${m.height}p${rate}` : `${m.width}×${m.height} ${rate}`
}

export function resolutionLabel(c: SourceCapabilitiesDto) {
  return `${c.max_width}×${c.max_height}`
}
