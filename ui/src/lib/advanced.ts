import type { AudioCodecChoice } from '@/types/generated/AudioCodecChoice'
import type { Container } from '@/types/generated/Container'
import type { EncoderChoice } from '@/types/generated/EncoderChoice'
import type { NodeDto } from '@/types/generated/NodeDto'
import type { OutputAdvanced } from '@/types/generated/OutputAdvanced'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'
import type { RateControl } from '@/types/generated/RateControl'
import type { SpeedPreset } from '@/types/generated/SpeedPreset'
import { CONTAINERS, hasBitrate } from '@/lib/codecs'

// Client-side copy of an output's Advanced settings logic, mirroring
// `RecordingProfile` in `node/src/pipeline/profile.rs` (encoder candidates,
// Auto audio, checks), so the editor can explain and validate without a
// round trip.

export const DEFAULT_QUALITY = 70
export const DEFAULT_KEYFRAME_SECS = 2
export const DEFAULT_SPEED: SpeedPreset = 'veryfast'

export function defaultAdvanced(): OutputAdvanced {
  return {
    encoder: 'auto',
    rate_control: 'average',
    quality: null,
    speed_preset: null,
    keyframe_secs: null,
    deinterlace: 'auto',
    audio_codec: 'auto',
    audio_bitrate_kbps: null,
    audio_channels: 'all',
    channel_pick: [],
    split_minutes: null,
    split_gb: null,
  }
}

// ── Encoders ──────────────────────────────────────────────────────────────────

const ENCODER_NAMES: Record<string, string> = {
  vtenc_h264: 'VideoToolbox',
  vtenc_h265: 'VideoToolbox',
  vtenc_prores: 'VideoToolbox',
  x264enc: 'x264',
  x265enc: 'x265',
  vp9enc: 'libvpx',
  avenc_prores_ks: 'FFmpeg ProRes',
  identity: 'none',
}
const HARDWARE = new Set(['vtenc_h264', 'vtenc_h265', 'vtenc_prores'])

export const isProRes = (leg: Pick<PresetOutputInput, 'codec'>) => leg.codec.startsWith('prores')

/** Encoder elements for a leg's codec, best first, before the Encoder choice filters them. */
function codecEncoders(leg: Pick<PresetOutputInput, 'codec' | 'chroma'>): string[] {
  const yuv420 = leg.chroma === '420'
  switch (leg.codec) {
    case 'h264':
      return yuv420 ? ['vtenc_h264', 'x264enc'] : ['x264enc']
    case 'h265':
      return yuv420 ? ['vtenc_h265', 'x265enc'] : ['x265enc']
    case 'vp9':
      return ['vp9enc']
    case 'uncompressed':
      return ['identity']
    default:
      return ['vtenc_prores', 'avenc_prores_ks']
  }
}

/** The encoders a leg may use, best first. */
export function legEncoders(leg: PresetOutputInput, choice: EncoderChoice = leg.advanced.encoder) {
  return codecEncoders(leg).filter((e) =>
    choice === 'hardware' ? HARDWARE.has(e) : choice === 'software' ? !HARDWARE.has(e) : true,
  )
}

/** Why an Encoder choice can't be used with this codec, or null. */
export function encoderChoiceProblem(leg: PresetOutputInput, choice: EncoderChoice): string | null {
  if (legEncoders(leg, choice).length) return null
  if (choice !== 'hardware') return 'No software encoder for this codec'
  if (leg.codec === 'h264' || leg.codec === 'h265') return 'Hardware encoders take 4:2:0 only'
  return 'No hardware encoder for this codec'
}

/** What a leg's encoder resolves to on each node that has reported its encoders. */
export function encoderOnNodes(leg: PresetOutputInput, nodes: NodeDto[]) {
  return nodes
    .filter((n) => n.encoders.length)
    .map((n) => {
      const e = legEncoders(leg).find((e) => e === 'identity' || n.encoders.includes(e))
      return {
        node: n.name,
        encoder: e ? `${ENCODER_NAMES[e]}${e === 'identity' ? '' : HARDWARE.has(e) ? ' (hardware)' : ' (software)'}` : 'unavailable',
      }
    })
}

/** Whether a leg can end up on a software x264/x265 encoder, where the speed preset applies. */
export function usesSpeedPreset(leg: PresetOutputInput) {
  return legEncoders(leg).some((e) => e === 'x264enc' || e === 'x265enc')
}

// ── Audio ─────────────────────────────────────────────────────────────────────

const AUDIO_FOR: Record<Exclude<AudioCodecChoice, 'auto'>, Container[]> = {
  pcm: ['mov', 'mkv'],
  aac: ['mov', 'mp4', 'mkv'],
  opus: ['mkv'],
}
const AUDIO_NAMES = { pcm: 'PCM 24-bit', aac: 'AAC', opus: 'Opus' } as const
export const AUDIO_DEFAULT_KBPS = { aac: 256, opus: 160 } as const

/** The audio codec a leg records, with Auto resolved. */
export function resolvedAudio(leg: PresetOutputInput): Exclude<AudioCodecChoice, 'auto'> {
  if (leg.advanced.audio_codec !== 'auto') return leg.advanced.audio_codec
  if (!hasBitrate(leg.codec)) return 'pcm'
  return leg.container === 'mkv' ? 'opus' : 'aac'
}

export function audioCodecProblem(choice: AudioCodecChoice, container: Container): string | null {
  if (choice === 'auto' || AUDIO_FOR[choice].includes(container)) return null
  return `${AUDIO_NAMES[choice]} can only be recorded to ${AUDIO_FOR[choice].map((c) => CONTAINERS[c]).join(' or ')}`
}

/** "1-2, 5" → [1, 2, 5]; null if it doesn't parse. */
export function parseChannels(text: string): number[] | null {
  const out: number[] = []
  for (const part of text.split(',').map((p) => p.trim()).filter(Boolean)) {
    const m = /^(\d+)(?:\s*-\s*(\d+))?$/.exec(part)
    if (!m) return null
    const [a, b] = [Number(m[1]), Number(m[2] ?? m[1])]
    if (a < 1 || b < a || b > 64) return null
    for (let c = a; c <= b; c++) out.push(c)
  }
  return out.length ? out : null
}

/** [1, 2, 5] → "1-2, 5" */
export function formatChannels(list: number[]) {
  const parts: string[] = []
  for (let i = 0; i < list.length; ) {
    let j = i
    while (j + 1 < list.length && list[j + 1] === list[j]! + 1) j++
    parts.push(j > i ? `${list[i]}-${list[j]}` : String(list[i]))
    i = j + 1
  }
  return parts.join(', ')
}

// ── Options ───────────────────────────────────────────────────────────────────

export const RATE_CONTROL_OPTIONS: { value: RateControl; label: string }[] = [
  { value: 'average', label: 'Average bitrate' },
  { value: 'constant', label: 'Constant bitrate' },
  { value: 'quality', label: 'Constant quality' },
]

export const SPEED_OPTIONS: { value: SpeedPreset; label: string }[] = [
  { value: 'ultrafast', label: 'ultrafast' },
  { value: 'superfast', label: 'superfast' },
  { value: 'veryfast', label: 'veryfast' },
  { value: 'faster', label: 'faster' },
  { value: 'fast', label: 'fast' },
  { value: 'medium', label: 'medium' },
  { value: 'slow', label: 'slow' },
]

export function audioCodecOptions(leg: PresetOutputInput) {
  return [
    { value: 'auto' as const, label: `Auto (${AUDIO_NAMES[resolvedAudio({ ...leg, advanced: { ...leg.advanced, audio_codec: 'auto' } })]})` },
    ...(['pcm', 'aac', 'opus'] as const).map((value) => {
      const reason = audioCodecProblem(value, leg.container)
      return { value, label: AUDIO_NAMES[value], disabled: !!reason, reason: reason ?? undefined }
    }),
  ]
}

// ── Checks and summary ────────────────────────────────────────────────────────

export type AdvancedField =
  | 'encoder'
  | 'quality'
  | 'keyframe_secs'
  | 'audio_codec'
  | 'audio_bitrate_kbps'
  | 'channel_pick'
  | 'split_minutes'
  | 'split_gb'

/** Client-side copies of the server's checks (`check_advanced`), for inline feedback. */
export function advancedProblems(leg: PresetOutputInput) {
  const a = leg.advanced
  const p: Partial<Record<AdvancedField, string>> = {}
  const enc = encoderChoiceProblem(leg, a.encoder)
  if (enc) p.encoder = enc
  if (a.quality != null && !(a.quality >= 1 && a.quality <= 100)) p.quality = 'Use 1–100'
  if (a.keyframe_secs != null && !(a.keyframe_secs >= 0.1 && a.keyframe_secs <= 60)) p.keyframe_secs = 'Use 0.1–60 seconds'
  if (a.audio_bitrate_kbps != null && !(a.audio_bitrate_kbps >= 32 && a.audio_bitrate_kbps <= 512))
    p.audio_bitrate_kbps = 'Use 32–512 kbps'
  if (a.split_minutes != null && !(Number.isInteger(a.split_minutes) && a.split_minutes >= 1 && a.split_minutes <= 1440))
    p.split_minutes = 'Use 1–1440 whole minutes'
  if (a.split_gb != null && !(a.split_gb >= 0.1 && a.split_gb <= 10000)) p.split_gb = 'Use 0.1–10,000 GB'
  const audio = audioCodecProblem(a.audio_codec, leg.container)
  if (audio) p.audio_codec = audio
  if (a.audio_channels === 'pick') {
    if (!a.channel_pick.length) p.channel_pick = 'List channels, e.g. 1-2 or 3, 4'
    else if (a.channel_pick.length > 2 && resolvedAudio(leg) !== 'pcm')
      p.channel_pick = 'AAC and Opus take 1 or 2 channels; use PCM for more'
  }
  return p
}

/** Short descriptions of every Advanced setting that differs from its default. */
export function advancedSummary(leg: PresetOutputInput): string[] {
  const a = leg.advanced
  const out: string[] = []
  if (a.encoder !== 'auto') out.push(a.encoder === 'hardware' ? 'Hardware' : 'Software')
  if (hasBitrate(leg.codec)) {
    if (a.rate_control === 'constant') out.push('Constant bitrate')
    if (a.rate_control === 'quality') out.push(`Quality ${a.quality ?? DEFAULT_QUALITY}`)
    if (a.speed_preset && usesSpeedPreset(leg)) out.push(a.speed_preset)
    if (a.keyframe_secs != null) out.push(`Keyframes ${a.keyframe_secs} s`)
  }
  if (a.deinterlace === 'off' && !isProRes(leg)) out.push('No deinterlace')
  if (a.audio_codec !== 'auto') out.push(AUDIO_NAMES[a.audio_codec])
  if (a.audio_bitrate_kbps != null && resolvedAudio(leg) !== 'pcm') out.push(`${a.audio_bitrate_kbps} kbps audio`)
  if (a.audio_channels === 'stereo') out.push('Stereo mix')
  if (a.audio_channels === 'pick') out.push(`Channels ${formatChannels(a.channel_pick) || '?'}`)
  const split = [a.split_minutes != null && formatMinutes(a.split_minutes), a.split_gb != null && `${a.split_gb} GB`].filter(Boolean)
  if (split.length) out.push(`Split ${split.join(' or ')}`)
  return out
}

/** 90 → "90 min", 120 → "2 h". */
export function formatMinutes(m: number) {
  return m % 60 === 0 && m >= 60 ? `${m / 60} h` : `${m} min`
}

export const SPLIT_MINUTE_PRESETS = [15, 30, 60, 120] as const

/** Drop choices a codec or container change made impossible, back to Auto. */
export function fitAdvanced(leg: PresetOutputInput): OutputAdvanced {
  const a = { ...leg.advanced }
  if (encoderChoiceProblem(leg, a.encoder)) a.encoder = 'auto'
  if (audioCodecProblem(a.audio_codec, leg.container)) a.audio_codec = 'auto'
  return a
}
