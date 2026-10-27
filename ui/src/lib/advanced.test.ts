import { describe, expect, it } from 'vitest'
import {
  type AdvancedField,
  advancedProblems,
  advancedSummary,
  audioCodecOptions,
  audioCodecProblem,
  defaultAdvanced,
  encoderChoiceProblem,
  encoderOnNodes,
  encoderWarning,
  fitAdvanced,
  formatChannels,
  formatMinutes,
  legEncoders,
  nodesWithoutEncoder,
  parseChannels,
  resolvedAudio,
  usesSpeedPreset,
} from '@/lib/advanced'
import advancedCases from '@/types/generated/advancedCases.json'
import type { NodeDto } from '@/types/generated/NodeDto'
import type { OutputAdvanced } from '@/types/generated/OutputAdvanced'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'

function leg(overrides: Partial<Omit<PresetOutputInput, 'advanced'>> = {}, advanced: Partial<OutputAdvanced> = {}): PresetOutputInput {
  return {
    name: 'Main',
    codec: 'h264',
    container: 'mp4',
    resolution: null,
    framerate: null,
    bitrate_kbps: null,
    chroma: '420',
    path_template: '~/rec/{source}.{ext}',
    ...overrides,
    advanced: { ...defaultAdvanced(), ...advanced },
  }
}

function node(name: string, encoders: string[]): NodeDto {
  return {
    url: '',
    healthy: true,
    is_self: false,
    fingerprint: null,
    unreachable: null,
    id: name,
    name,
    version: '0.2.0',
    uptime_secs: 0,
    is_controller: false,
    encoders,
    source_types: [],
    clock: null,
    replay_codecs: [],
    busy: null,
  }
}

const mac = node('mac', ['vtenc_h264', 'vtenc_h265', 'vtenc_prores', 'x264enc'])
const linux = node('linux', ['x264enc', 'x265enc', 'vp9enc', 'avenc_prores_ks'])
const bare = node('bare', ['vp9enc'])
const old = node('old', [])

describe('legEncoders', () => {
  it('lists hardware first for 4:2:0 H.264 and H.265', () => {
    expect(legEncoders(leg())).toEqual(['vtenc_h264', 'vah264enc', 'x264enc'])
    expect(legEncoders(leg({ codec: 'h265' }))).toEqual(['vtenc_h265', 'vah265enc', 'x265enc'])
  })
  it('has only software for other chroma', () => {
    expect(legEncoders(leg({ chroma: '422' }))).toEqual(['x264enc'])
    expect(legEncoders(leg({ codec: 'h265', chroma: '444' }))).toEqual(['x265enc'])
  })
  it('covers the other codecs', () => {
    expect(legEncoders(leg({ codec: 'vp9' }))).toEqual(['vp9enc'])
    expect(legEncoders(leg({ codec: 'prores_422hq' }))).toEqual(['vtenc_prores', 'avenc_prores_ks'])
    expect(legEncoders(leg({ codec: 'uncompressed' }))).toEqual(['identity'])
  })
  it('filters by the Encoder choice', () => {
    expect(legEncoders(leg({}, { encoder: 'hardware' }))).toEqual(['vtenc_h264', 'vah264enc'])
    expect(legEncoders(leg({}, { encoder: 'software' }))).toEqual(['x264enc'])
    expect(legEncoders(leg(), 'software')).toEqual(['x264enc'])
  })
})

describe('encoderChoiceProblem', () => {
  it('allows a choice with encoders', () => {
    expect(encoderChoiceProblem(leg(), 'hardware')).toBeNull()
    expect(encoderChoiceProblem(leg({ codec: 'uncompressed' }), 'software')).toBeNull()
  })
  it('explains a choice without', () => {
    expect(encoderChoiceProblem(leg({ chroma: '422' }), 'hardware')).toBe('Hardware encoders take 4:2:0 only')
    expect(encoderChoiceProblem(leg({ codec: 'vp9' }), 'hardware')).toBe('No hardware encoder for this codec')
    expect(encoderChoiceProblem(leg({ codec: 'uncompressed' }), 'hardware')).toBe('No hardware encoder for this codec')
  })
})

describe('encoders on nodes', () => {
  it('says what each reporting node resolves to', () => {
    expect(encoderOnNodes(leg(), [mac, linux, bare, old])).toEqual([
      { node: 'mac', encoder: 'VideoToolbox (hardware)' },
      { node: 'linux', encoder: 'x264 (software)' },
      { node: 'bare', encoder: 'unavailable' },
    ])
    expect(encoderOnNodes(leg({ codec: 'prores_422' }), [linux])).toEqual([{ node: 'linux', encoder: 'FFmpeg ProRes (software)' }])
  })
  it('needs no encoder for uncompressed', () => {
    expect(encoderOnNodes(leg({ codec: 'uncompressed', container: 'mov' }), [bare])).toEqual([{ node: 'bare', encoder: 'none' }])
    expect(nodesWithoutEncoder(leg({ codec: 'uncompressed', container: 'mov' }), [bare])).toEqual([])
  })
  it('lists reporting nodes without one', () => {
    expect(nodesWithoutEncoder(leg(), [mac, linux, bare, old])).toEqual(['bare'])
    expect(nodesWithoutEncoder(leg({}, { encoder: 'hardware' }), [mac, linux, bare, old])).toEqual(['linux', 'bare'])
  })
  it('warns about them', () => {
    expect(encoderWarning(leg(), [mac, linux])).toBeNull()
    expect(encoderWarning(leg(), [mac, bare])).toBe('Auto: no encoder on bare')
    expect(encoderWarning(leg({}, { encoder: 'hardware' }), [mac, linux, bare])).toBe('Hardware: no hardware encoder on linux, bare')
    expect(encoderWarning(leg({ codec: 'h265' }, { encoder: 'software' }), [mac])).toBe('Software: no software encoder on mac')
  })
})

describe('usesSpeedPreset', () => {
  it('is for x264 and x265 only', () => {
    expect(usesSpeedPreset(leg())).toBe(true)
    expect(usesSpeedPreset(leg({ codec: 'h265' }, { encoder: 'software' }))).toBe(true)
    expect(usesSpeedPreset(leg({}, { encoder: 'hardware' }))).toBe(false)
    expect(usesSpeedPreset(leg({ codec: 'vp9', container: 'mkv' }))).toBe(false)
  })
})

describe('audio', () => {
  it('resolves Auto by codec and container', () => {
    expect(resolvedAudio(leg())).toBe('aac')
    expect(resolvedAudio(leg({ container: 'mkv' }))).toBe('opus')
    expect(resolvedAudio(leg({ codec: 'prores_422', container: 'mov' }))).toBe('pcm')
    expect(resolvedAudio(leg({ codec: 'uncompressed', container: 'mkv' }))).toBe('pcm')
    expect(resolvedAudio(leg({}, { audio_codec: 'pcm' }))).toBe('pcm')
  })
  it('checks the codec against the container', () => {
    expect(audioCodecProblem('auto', 'mp4')).toBeNull()
    expect(audioCodecProblem('aac', 'mkv')).toBeNull()
    expect(audioCodecProblem('pcm', 'mp4')).toBe('PCM 24-bit can only be recorded to .mov or .mkv')
    expect(audioCodecProblem('opus', 'mov')).toBe('Opus can only be recorded to .mkv')
  })
  it('offers Auto with what it resolves to', () => {
    expect(audioCodecOptions(leg({ container: 'mp4' }, { audio_codec: 'pcm' }))).toEqual([
      { value: 'auto', label: 'Auto (AAC)' },
      { value: 'pcm', label: 'PCM 24-bit', disabled: true, reason: 'PCM 24-bit can only be recorded to .mov or .mkv' },
      { value: 'aac', label: 'AAC', disabled: false, reason: undefined },
      { value: 'opus', label: 'Opus', disabled: true, reason: 'Opus can only be recorded to .mkv' },
    ])
    expect(audioCodecOptions(leg({ container: 'mkv' }))[0]!.label).toBe('Auto (Opus)')
  })
})

describe('channels', () => {
  it('parses lists and ranges', () => {
    expect(parseChannels('1-2, 5')).toEqual([1, 2, 5])
    expect(parseChannels('3 - 4,1')).toEqual([3, 4, 1])
    expect(parseChannels('1,,2,')).toEqual([1, 2])
    expect(parseChannels('1-64')).toHaveLength(64)
  })
  it('rejects bad lists', () => {
    for (const bad of ['', ' , ', '0', '2-1', '65', '1-65', 'a', '1-', '1.5']) expect(parseChannels(bad), bad).toBeNull()
  })
  it('formats runs as ranges', () => {
    expect(formatChannels([1, 2, 5])).toBe('1-2, 5')
    expect(formatChannels([3, 1, 2])).toBe('3, 1-2')
    expect(formatChannels([7])).toBe('7')
    expect(formatChannels([])).toBe('')
  })
  it('round-trips', () => {
    for (const text of ['1-2, 5', '1, 3-6, 8']) expect(formatChannels(parseChannels(text)!)).toBe(text)
  })
})

describe('advancedProblems', () => {
  it('passes the defaults', () => {
    expect(advancedProblems(leg())).toEqual({})
  })
  it('checks each range', () => {
    const p = advancedProblems(
      leg({}, { quality: 0, keyframe_secs: 0.05, audio_bitrate_kbps: 600, split_minutes: 1.5, split_gb: 0 }),
    )
    expect(p).toEqual({
      quality: 'Use 1–100',
      keyframe_secs: 'Use 0.1–60 seconds',
      audio_bitrate_kbps: 'Use 32–512 kbps',
      split_minutes: 'Use 1–1440 whole minutes',
      split_gb: 'Use 0.1–10,000 GB',
    })
  })
  it('accepts the range ends', () => {
    expect(advancedProblems(leg({}, { quality: 100, keyframe_secs: 60, audio_bitrate_kbps: 32, split_minutes: 1440, split_gb: 10000 }))).toEqual(
      {},
    )
  })
  it('checks the encoder and audio codec', () => {
    expect(advancedProblems(leg({ chroma: '422' }, { encoder: 'hardware', audio_codec: 'pcm' }))).toEqual({
      encoder: 'Hardware encoders take 4:2:0 only',
      audio_codec: 'PCM 24-bit can only be recorded to .mov or .mkv',
    })
  })
  it('flags the same legs as the server', () => {
    const cases = advancedCases as { name: string; leg: PresetOutputInput; error: AdvancedField | null }[]
    expect(cases.length).toBeGreaterThan(0)
    for (const c of cases) expect(Object.keys(advancedProblems(c.leg)), c.name).toEqual(c.error ? [c.error] : [])
  })
  it('wants whole numbers where the server does', () => {
    expect(advancedProblems(leg({}, { quality: 70.5, audio_bitrate_kbps: 128.5, channel_pick: [1.5], audio_channels: 'pick' }))).toEqual({
      quality: 'Use 1–100',
      audio_bitrate_kbps: 'Use 32–512 kbps',
      channel_pick: 'Channels are numbered 1–64',
    })
  })
  it('checks picked channels', () => {
    expect(advancedProblems(leg({}, { audio_channels: 'pick' })).channel_pick).toBe('List channels, e.g. 1-2 or 3, 4')
    expect(advancedProblems(leg({}, { audio_channels: 'pick', channel_pick: [1, 2, 3] })).channel_pick).toBe(
      'AAC and Opus take 1 or 2 channels; use PCM for more',
    )
    expect(advancedProblems(leg({}, { audio_channels: 'pick', channel_pick: [0] })).channel_pick).toBe('Channels are numbered 1–64')
    expect(advancedProblems(leg({ codec: 'prores_422', container: 'mov' }, { audio_channels: 'pick', channel_pick: [1, 2, 3] }))).toEqual({})
  })
})

describe('advancedSummary', () => {
  it('is empty for the defaults', () => {
    expect(advancedSummary(leg())).toEqual([])
  })
  it('describes what differs', () => {
    const l = leg(
      {},
      {
        encoder: 'software',
        rate_control: 'quality',
        speed_preset: 'slow',
        keyframe_secs: 1,
        deinterlace: 'off',
        audio_codec: 'aac',
        audio_bitrate_kbps: 192,
        audio_channels: 'pick',
        channel_pick: [1, 2],
        split_minutes: 90,
        split_gb: 4,
      },
    )
    expect(advancedSummary(l)).toEqual([
      'Software',
      'Quality 70',
      'slow',
      'Keyframes 1 s',
      'No deinterlace',
      'AAC',
      '192 kbps audio',
      'Channels 1-2',
      'Split 90 min or 4 GB',
    ])
  })
  it("leaves out settings that don't apply", () => {
    const prores = leg(
      { codec: 'prores_422', container: 'mov' },
      { rate_control: 'constant', keyframe_secs: 1, deinterlace: 'off', audio_bitrate_kbps: 192, speed_preset: 'slow' },
    )
    expect(advancedSummary(prores)).toEqual([])
    expect(advancedSummary(leg({}, { encoder: 'hardware', speed_preset: 'slow' }))).toEqual(['Hardware'])
  })
  it('shows other choices', () => {
    expect(advancedSummary(leg({}, { rate_control: 'constant', audio_channels: 'stereo', split_minutes: 120 }))).toEqual([
      'Constant bitrate',
      'Stereo mix',
      'Split 2 h',
    ])
  })
})

describe('formatMinutes', () => {
  it('uses hours for whole hours', () => {
    expect(formatMinutes(30)).toBe('30 min')
    expect(formatMinutes(90)).toBe('90 min')
    expect(formatMinutes(120)).toBe('2 h')
  })
})

describe('fitAdvanced', () => {
  it('drops impossible choices back to Auto', () => {
    const l = leg({ chroma: '422' }, { encoder: 'hardware', audio_codec: 'opus', quality: 80 })
    expect(fitAdvanced(l)).toEqual({ ...l.advanced, encoder: 'auto', audio_codec: 'auto' })
    expect(l.advanced.encoder).toBe('hardware')
  })
  it('keeps possible ones', () => {
    const l = leg({ container: 'mkv' }, { encoder: 'hardware', audio_codec: 'opus' })
    expect(fitAdvanced(l)).toEqual(l.advanced)
  })
})
