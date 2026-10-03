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

// Containers each codec can be recorded to. Mirrors `incompatible()` in
// `node/src/pipeline/profile.rs`, which rejects the rest when a preset is saved.
const ALL: Container[] = ['mov', 'mp4', 'mkv']
const MOV_MKV: Container[] = ['mov', 'mkv']
export const CONTAINERS_FOR: Record<VideoCodec, Container[]> = {
  h264: ALL,
  h265: ALL,
  vp9: ['mkv'],
  prores_4444: MOV_MKV,
  prores_422hq: MOV_MKV,
  prores_422: MOV_MKV,
  prores_422lt: MOV_MKV,
  prores_422proxy: MOV_MKV,
  uncompressed: MOV_MKV,
}

/** Why `codec` can't go in `container`, or null if it can. Same wording as the server. */
export function incompatibleReason(codec: VideoCodec, container: Container): string | null {
  if (CONTAINERS_FOR[codec].includes(container)) return null
  if (codec === 'vp9') return 'VP9 can only be recorded to .mkv'
  if (codec.startsWith('prores')) return 'ProRes can only be recorded to .mov or .mkv'
  return 'uncompressed video can only be recorded to .mov or .mkv'
}

/** Every container, with the ones `codec` can't use disabled and explained. */
export function containerOptions(codec: VideoCodec) {
  return (Object.keys(CONTAINERS) as Container[]).map((value) => {
    const reason = incompatibleReason(codec, value)
    return { value, label: CONTAINERS[value], disabled: !!reason, reason: reason ?? undefined }
  })
}

export const CODEC_OPTIONS = (Object.entries(CODECS) as [VideoCodec, string][]).map(([value, label]) => ({ value, label }))

export const CHROMA_OPTIONS: { value: ChromaSubsampling; label: string }[] = [
  { value: '420', label: '4:2:0 — plays everywhere' },
  { value: '422', label: '4:2:2' },
  { value: '444', label: '4:4:4' },
]

// ── Path templates ────────────────────────────────────────────────────────────

/** Tokens a path template can use; expanded on the recording node (see `plan_legs`). */
export const PATH_TOKENS = [
  { token: '{source}', help: 'Source ID' },
  { token: '{node}', help: 'Recording node name' },
  { token: '{date}', help: 'Start date, YYYY-MM-DD' },
  { token: '{datetime}', help: 'Start time, YYYYMMDD_HHMMSS' },
  { token: '{output}', help: "This output's name" },
  { token: '{ext}', help: 'File extension from the container' },
] as const

const pad = (n: number) => String(n).padStart(2, '0')

/** A template expanded the way the node would expand it at `at`. `~` stays (the node's home). */
export function expandPath(
  leg: Pick<PresetOutputInput, 'path_template' | 'name' | 'container'>,
  vars: { source: string; node: string },
  at = new Date(),
) {
  const date = `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}`
  const datetime = `${at.getFullYear()}${pad(at.getMonth() + 1)}${pad(at.getDate())}_${pad(at.getHours())}${pad(at.getMinutes())}${pad(at.getSeconds())}`
  return leg.path_template
    .replaceAll('{output}', leg.name)
    .replaceAll('{ext}', leg.container)
    .replaceAll('{source}', vars.source)
    .replaceAll('{node}', vars.node)
    .replaceAll('{date}', date)
    .replaceAll('{datetime}', datetime)
}

/** Client-side copies of the server's format checks, for inline feedback. */
export function legProblems(leg: PresetOutputInput) {
  const p: Partial<Record<'resolution' | 'framerate' | 'path', string>> = {}
  const res = leg.resolution?.trim()
  if (res && !/^\s*[1-9]\d*\s*[xX]\s*[1-9]\d*\s*$/.test(res)) p.resolution = 'Use WIDTHxHEIGHT, e.g. 1920x1080'
  const fps = leg.framerate?.trim()
  if (fps && !/^\s*[1-9]\d*\s*(\/\s*[1-9]\d*\s*)?$/.test(fps)) p.framerate = 'Use a number or a fraction, e.g. 30 or 30000/1001'
  if (!leg.path_template.trim()) p.path = 'A path is required'
  return p
}

/** Indexes of legs that would write the same file (same template after {output}/{ext}). */
export function clashingLegs(legs: PresetOutputInput[]) {
  const seen = new Map<string, number>()
  const clash = new Set<number>()
  legs.forEach((l, i) => {
    const key = l.path_template.replaceAll('{output}', l.name).replaceAll('{ext}', l.container)
    if (seen.has(key)) {
      clash.add(i)
      clash.add(seen.get(key)!)
    } else seen.set(key, i)
  })
  return clash
}
