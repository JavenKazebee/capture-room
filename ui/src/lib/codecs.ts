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

/** Codecs that take a target bitrate (ProRes and uncompressed are fixed-rate by design). */
export function hasBitrate(codec: VideoCodec) {
  return codec === 'h264' || codec === 'h265' || codec === 'vp9'
}

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
    leg.framerate ? `${framerateLabel(leg.framerate)} fps` : 'source fps',
    leg.bitrate_kbps && hasBitrate(leg.codec) ? `${leg.bitrate_kbps} kbps` : null,
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
  { value: '420', label: '4:2:0' },
  { value: '422', label: '4:2:2' },
  { value: '444', label: '4:4:4' },
]

// ── Resolution / frame rate ───────────────────────────────────────────────────

/** Common output sizes; anything else is entered as a custom WIDTHxHEIGHT. */
export const RESOLUTION_PRESETS = [
  { value: '3840x2160', label: '3840 × 2160 (UHD)' },
  { value: '2560x1440', label: '2560 × 1440' },
  { value: '1920x1080', label: '1920 × 1080 (HD)' },
  { value: '1280x720', label: '1280 × 720' },
] as const

/** Common frame rates, stored as the fractions the node records at. */
export const FRAMERATE_PRESETS = [
  { value: '24000/1001', label: '23.976' },
  { value: '24', label: '24' },
  { value: '25', label: '25' },
  { value: '30000/1001', label: '29.97' },
  { value: '30', label: '30' },
  { value: '50', label: '50' },
  { value: '60000/1001', label: '59.94' },
  { value: '60', label: '60' },
] as const

export function parseResolution(s: string): [number, number] | null {
  const m = /^\s*([1-9]\d*)\s*[xX]\s*([1-9]\d*)\s*$/.exec(s)
  return m ? [Number(m[1]), Number(m[2])] : null
}

const gcd = (a: number, b: number): number => (b ? gcd(b, a % b) : a)

/**
 * "30" → [30, 1]; "30000/1001" → [30000, 1001]; "29.97" → [30000, 1001].
 * Mirrors `parse_framerate` in `node/src/pipeline/profile.rs`.
 */
export function parseFramerate(input: string): [number, number] | null {
  const s = input.trim()
  if (s.includes('.') && !s.includes('/')) {
    if (!/^\d*\.\d*$/.test(s)) return null
    const fps = Number(s)
    if (!Number.isFinite(fps) || fps <= 0 || fps > 1000) return null
    const whole = Math.round(fps)
    if (Math.abs(fps - whole) < 1e-9) return [whole, 1]
    const ntsc = Math.round(fps * 1.001)
    if (Math.abs(fps - (ntsc * 1000) / 1001) < 0.006) return [ntsc * 1000, 1001]
    const milli = Math.round(fps * 1000)
    const g = gcd(milli, 1000)
    return milli > 0 ? [milli / g, 1000 / g] : null
  }
  const m = /^([1-9]\d*)\s*(?:\/\s*([1-9]\d*))?$/.exec(s)
  return m ? [Number(m[1]), Number(m[2] ?? 1)] : null
}

/** [30000, 1001] → "29.97", as `{fps}` and labels show it. */
export function formatFramerate([n, d]: [number, number]) {
  return (n / d).toFixed(3).replace(/\.?0+$/, '')
}

/** A stored frame rate shown as a decimal ("30000/1001" → "29.97"). */
export function framerateLabel(value: string) {
  const f = parseFramerate(value)
  return f ? formatFramerate(f) : value
}

/** The preset matching a stored frame rate, comparing values ("29.97" matches "30000/1001"). */
export function framerateChoice(value: string) {
  const f = parseFramerate(value)
  return f ? FRAMERATE_PRESETS.find((p) => { const q = parseFramerate(p.value)!; return q[0] * f[1] === f[0] * q[1] })?.value : undefined
}

// ── Path templates ────────────────────────────────────────────────────────────

/** Tokens a path template can use, grouped for the editor; expanded on the recording node (see `plan_legs`). */
export const PATH_TOKEN_GROUPS = [
  {
    label: 'Who',
    tokens: [
      { token: '{source}', help: 'Source ID' },
      { token: '{source_name}', help: "Source's display name" },
      { token: '{node}', help: 'Recording node name' },
    ],
  },
  {
    label: 'When',
    tokens: [
      { token: '{date}', help: 'Start date, YYYY-MM-DD' },
      { token: '{time}', help: 'Start time, HHMMSS' },
      { token: '{datetime}', help: 'Start date and time, YYYYMMDD_HHMMSS' },
      { token: '{year}', help: 'Start year, YYYY' },
      { token: '{month}', help: 'Start month, MM' },
      { token: '{day}', help: 'Start day, DD' },
    ],
  },
  {
    label: 'What',
    tokens: [
      { token: '{preset}', help: 'Preset name' },
      { token: '{output}', help: "This output's name" },
      { token: '{codec}', help: 'Codec, e.g. h264 or prores_422hq' },
      { token: '{resolution}', help: "Output size, e.g. 1920x1080 (the source's when matching it)" },
      { token: '{fps}', help: "Frame rate, e.g. 29.97 (the source's when matching it)" },
      { token: '{ext}', help: 'File extension from the container' },
    ],
  },
  {
    label: 'Numbering',
    tokens: [
      { token: '{take}', help: "01, 02, … — the first number whose files don't exist yet, so nothing is overwritten" },
      { token: '{segment}', help: 'File number within a split recording: 001, 002, … Added before the extension automatically when an output splits' },
    ],
  },
] as const

export type PathToken = (typeof PATH_TOKEN_GROUPS)[number]['tokens'][number]

const KNOWN_TOKENS = new Set<string>(PATH_TOKEN_GROUPS.flatMap((g) => g.tokens.map((t) => t.token)))

/** The first `{...}` that isn't a token; mirrors `unknown_token` in `profile.rs`. */
export function unknownToken(template: string) {
  // The innermost braces: in "{a{date}", "{date}" is what gets checked.
  return template.match(/\{[^{}]*\}/g)?.find((t) => !KNOWN_TOKENS.has(t)) ?? null
}

/** Where a template splits into the editor's two fields, and the extension it always ends with. */
export const EXT_SUFFIX = '.{ext}'

/**
 * A template as a folder and a file name. Token values never contain `/`, so
 * the last `/` always separates them. `name` is null when the template
 * doesn't end in `.{ext}`, which the split editor can't show.
 */
export function splitTemplate(t: string) {
  const slash = t.lastIndexOf('/')
  const folder = slash === 0 ? '/' : t.slice(0, Math.max(slash, 0))
  const file = t.slice(slash + 1)
  return { folder, name: file.endsWith(EXT_SUFFIX) ? file.slice(0, -EXT_SUFFIX.length) : null, file }
}

export function joinTemplate(folder: string, name: string) {
  const dir = folder === '/' ? '/' : folder.replace(/\/+$/, '')
  return `${dir && dir !== '/' ? `${dir}/` : dir}${name}${EXT_SUFFIX}`
}

/** A template forced to end in `.{ext}`: a literal extension (`.mov`) is swapped for it. */
export function withExtToken(t: string) {
  const { folder, name, file } = splitTemplate(t)
  return name !== null ? t : joinTemplate(folder, file.replace(/\.[A-Za-z0-9]{1,5}$/, ''))
}

/**
 * Common layouts, written below the template's root: the leading folders
 * without tokens (`~/capture-room`, `/Volumes/RAID/shoots`), which a pattern
 * keeps so picking one doesn't move recordings to another drive.
 */
export const PATH_PATTERNS = [
  { label: 'By date', path: '{date}/{source}_{time}' },
  { label: 'By date, then source', path: '{date}/{source_name}/{source}_{time}' },
  { label: 'By year / month / day', path: '{year}/{month}/{day}/{source}_{time}' },
  { label: 'By source', path: '{source_name}/{date}_{time}' },
  { label: 'By preset', path: '{preset}/{date}/{source}_{time}' },
  { label: 'Numbered takes', path: '{date}/{source}_take{take}' },
] as const

export function templateRoot(t: string) {
  const { folder } = splitTemplate(t)
  const parts = folder.split('/')
  const i = parts.findIndex((p) => p.includes('{'))
  const root = (i < 0 ? parts : parts.slice(0, i)).join('/')
  return root || (folder.startsWith('/') ? '/' : '~/capture-room')
}

/** A pattern applied below `t`'s root; several outputs also get `{output}` so they don't clash. */
export function applyPattern(t: string, path: string, multipleOutputs: boolean) {
  const root = templateRoot(t)
  const rel = multipleOutputs ? `${path}_{output}` : path
  return `${root === '/' ? '' : root}/${rel}${EXT_SUFFIX}`
}

const pad = (n: number) => String(n).padStart(2, '0')

/** A name made safe as one path component; mirrors `sanitize` in `profile.rs`. */
function sanitize(value: string) {
  const s = value.trim().replace(/[\x00-\x1f\x7f/\\:*?"<>|]/g, '-')
  return s === '' || s === '.' || s === '..' ? '_' : s
}

/** Whether an output starts new files as it goes (Advanced → Split). */
export function splits(leg: Pick<PresetOutputInput, 'advanced'>) {
  return leg.advanced.split_minutes != null || leg.advanced.split_gb != null
}

/**
 * A leg's template as recorded: a splitting leg without `{segment}` gets it
 * before the file name's extension. Mirrors `with_segment` in `profile.rs`.
 */
function segmentedTemplate(leg: Pick<PresetOutputInput, 'path_template' | 'advanced'>) {
  const t = leg.path_template
  if (!splits(leg) || t.includes('{segment}')) return t
  const nameStart = t.lastIndexOf('/') + 1
  const dot = t.slice(nameStart).lastIndexOf('.')
  return dot > 0 ? `${t.slice(0, nameStart + dot)}_{segment}${t.slice(nameStart + dot)}` : `${t}_{segment}`
}

/** Tokens an output settles by itself, whoever records it. */
function legTokens(leg: Pick<PresetOutputInput, 'name' | 'container' | 'codec' | 'resolution' | 'framerate'>) {
  const res = leg.resolution ? parseResolution(leg.resolution) : null
  const fps = leg.framerate ? parseFramerate(leg.framerate) : null
  return [
    ['{output}', sanitize(leg.name)],
    ['{ext}', leg.container],
    ['{codec}', leg.codec],
    ...(res ? [['{resolution}', `${res[0]}x${res[1]}`]] : []),
    ...(fps ? [['{fps}', formatFramerate(fps)]] : []),
    // The first file's number: what clashes and previews compare.
    ['{segment}', '001'],
  ] as [string, string][]
}

const expand = (template: string, pairs: [string, string][]) => pairs.reduce((t, [k, v]) => t.replaceAll(k, v), template)

/** A template expanded the way the node would expand it at `at`. `~` stays (the node's home). */
export function expandPath(
  leg: PresetOutputInput,
  vars: { source: string; sourceName: string; node: string; preset: string },
  at = new Date(),
) {
  const [y, mo, d] = [String(at.getFullYear()), pad(at.getMonth() + 1), pad(at.getDate())]
  const time = `${pad(at.getHours())}${pad(at.getMinutes())}${pad(at.getSeconds())}`
  return expand(segmentedTemplate(leg), [
    ...legTokens(leg),
    ['{source}', sanitize(vars.source)],
    ['{source_name}', sanitize(vars.sourceName)],
    ['{node}', sanitize(vars.node)],
    ['{preset}', sanitize(vars.preset || 'preset')],
    ['{date}', `${y}-${mo}-${d}`],
    ['{time}', time],
    ['{datetime}', `${y}${mo}${d}_${time}`],
    ['{year}', y],
    ['{month}', mo],
    ['{day}', d],
    ['{take}', '01'],
    ['{resolution}', 'source'],
    ['{fps}', 'source'],
  ])
}

/** Client-side copies of the server's format checks, for inline feedback. */
export function legProblems(leg: PresetOutputInput) {
  const p: Partial<Record<'resolution' | 'framerate' | 'path', string>> = {}
  const res = leg.resolution?.trim()
  if (res && !parseResolution(res)) p.resolution = 'Enter a width and height'
  const fps = leg.framerate?.trim()
  if (fps && !parseFramerate(fps)) p.framerate = 'Use a number or a fraction, e.g. 29.97 or 30000/1001'
  const unknown = unknownToken(leg.path_template)
  if (!leg.path_template.trim()) p.path = 'A path is required'
  else if (splitTemplate(leg.path_template).name === '') p.path = 'A file name is required'
  else if (unknown) p.path = `${unknown} isn't a token — check the spelling, or pick one from the list`
  return p
}

/** Indexes of legs that would write the same file (same template after the tokens each leg settles). */
export function clashingLegs(legs: PresetOutputInput[]) {
  const seen = new Map<string, number>()
  const clash = new Set<number>()
  legs.forEach((l, i) => {
    const key = expand(segmentedTemplate(l), legTokens(l))
    if (seen.has(key)) {
      clash.add(i)
      clash.add(seen.get(key)!)
    } else seen.set(key, i)
  })
  return clash
}
