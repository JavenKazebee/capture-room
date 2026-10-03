import type { ChromaSubsampling } from '@/types/generated/ChromaSubsampling'
import type { Container } from '@/types/generated/Container'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'
import type { VideoCodec } from '@/types/generated/VideoCodec'

// Records keyed by the generated unions, so a codec or container added in Rust
// fails the type check here until it gets a label.
export const CODECS: Record<VideoCodec, string> = {
  h264: 'H.264',
  h265: 'H.265 / HEVC',
  vp9: 'VP9',
  prores_4444: 'ProRes 4444',
  prores_422hq: 'ProRes 422 HQ',
  prores_422: 'ProRes 422',
  prores_422lt: 'ProRes 422 LT',
  prores_422proxy: 'ProRes 422 Proxy',
  uncompressed: 'Uncompressed',
}

export const CONTAINERS: Record<Container, string> = { mov: '.mov', mp4: '.mp4', mkv: '.mkv' }

/** Codecs whose chroma subsampling is configurable (ProRes picks it via the codec). */
export function hasChroma(codec: VideoCodec) {
  return codec === 'h264' || codec === 'h265'
}

export function chromaLabel(c: ChromaSubsampling) {
  return `${c[0]}:${c[1]}:${c[2]}`
}

/** One line describing an output leg: codec, container, format, bitrate. */
export function legSummary(leg: PresetOutputInput) {
  return [
    `${CODECS[leg.codec]} ${CONTAINERS[leg.container]}`,
    leg.resolution ?? 'source res',
    leg.framerate ? `${leg.framerate} fps` : 'source fps',
    leg.bitrate_kbps ? `${leg.bitrate_kbps} kbps` : null,
    hasChroma(leg.codec) ? chromaLabel(leg.chroma) : null,
  ]
    .filter(Boolean)
    .join(' · ')
}
