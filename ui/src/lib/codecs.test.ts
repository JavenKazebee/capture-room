import { describe, expect, it } from 'vitest'
import { defaultAdvanced } from '@/lib/advanced'
import {
  applyPattern,
  browserPlayable,
  clashingLegs,
  containerOptions,
  expandPath,
  formatFramerate,
  framerateChoice,
  framerateLabel,
  incompatibleReason,
  joinTemplate,
  legProblems,
  legSummary,
  parseFramerate,
  parseResolution,
  splitTemplate,
  templateRoot,
  unknownToken,
  withExtToken,
} from '@/lib/codecs'
import type { Container } from '@/types/generated/Container'
import type { OutputAdvanced } from '@/types/generated/OutputAdvanced'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'
import type { VideoCodec } from '@/types/generated/VideoCodec'

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

describe('legSummary', () => {
  it('describes a bitrate codec', () => {
    expect(legSummary(leg({ framerate: '30000/1001', bitrate_kbps: 8000 }))).toBe(
      'H.264 .mp4 · source res · 29.97 fps · 8000 kbps · 4:2:0',
    )
  })
  it('leaves out bitrate and chroma where the codec fixes them', () => {
    expect(legSummary(leg({ codec: 'prores_422hq', container: 'mov', resolution: '1920x1080', bitrate_kbps: 8000 }))).toBe(
      'ProRes 422 HQ .mov · 1920x1080 · source fps',
    )
  })
})

describe('incompatibleReason and containerOptions', () => {
  const allowed = (codec: VideoCodec) => containerOptions(codec).filter((o) => !o.disabled).map((o) => o.value)

  it('lets H.264 and H.265 go anywhere', () => {
    expect(allowed('h264')).toEqual(['mov', 'mp4', 'mkv'])
    expect(allowed('h265')).toEqual(['mov', 'mp4', 'mkv'])
  })
  it('keeps VP9 to .mkv', () => {
    expect(allowed('vp9')).toEqual(['mkv'])
    expect(incompatibleReason('vp9', 'mov')).toBe('VP9 can only be recorded to .mkv')
  })
  it('keeps ProRes and uncompressed out of .mp4', () => {
    for (const codec of ['prores_4444', 'prores_422hq', 'prores_422', 'prores_422lt', 'prores_422proxy'] as const) {
      expect(allowed(codec)).toEqual(['mov', 'mkv'])
      expect(incompatibleReason(codec, 'mp4')).toBe('ProRes can only be recorded to .mov or .mkv')
    }
    expect(allowed('uncompressed')).toEqual(['mov', 'mkv'])
    expect(incompatibleReason('uncompressed', 'mp4')).toBe('uncompressed video can only be recorded to .mov or .mkv')
  })
  it('labels every container and explains the disabled ones', () => {
    expect(containerOptions('vp9')).toEqual([
      { value: 'mov', label: '.mov', disabled: true, reason: 'VP9 can only be recorded to .mkv' },
      { value: 'mp4', label: '.mp4', disabled: true, reason: 'VP9 can only be recorded to .mkv' },
      { value: 'mkv', label: '.mkv', disabled: false, reason: undefined },
    ])
  })
})

describe('parseResolution', () => {
  it('reads WIDTHxHEIGHT', () => {
    expect(parseResolution('1920x1080')).toEqual([1920, 1080])
    expect(parseResolution(' 1280 X 720 ')).toEqual([1280, 720])
  })
  it('rejects zero, missing or junk sizes', () => {
    for (const bad of ['', '1920', '1920x', '0x720', '1920x0', '01920x1080', '-1x1', '1920x1080p']) expect(parseResolution(bad), bad).toBeNull()
  })
})

describe('parseFramerate', () => {
  it('reads whole numbers and fractions', () => {
    expect(parseFramerate('30')).toEqual([30, 1])
    expect(parseFramerate('30000/1001')).toEqual([30000, 1001])
    expect(parseFramerate(' 30 / 1 ')).toEqual([30, 1])
  })
  it('reads NTSC decimals as their fractions', () => {
    expect(parseFramerate('29.97')).toEqual([30000, 1001])
    expect(parseFramerate('23.976')).toEqual([24000, 1001])
    expect(parseFramerate('59.94')).toEqual([60000, 1001])
  })
  it('reads other decimals', () => {
    expect(parseFramerate('25.0')).toEqual([25, 1])
    expect(parseFramerate('12.5')).toEqual([25, 2])
    expect(parseFramerate('0.25')).toEqual([1, 4])
  })
  it('rejects zero, negatives, out-of-range and junk', () => {
    for (const bad of ['', 'abc', '0', '0.0', '-30', '30/0', '0/1', '1.5/2', '1000.5', '.', '1e3', '29.97fps']) expect(parseFramerate(bad), bad).toBeNull()
  })
})

describe('frame rate labels', () => {
  it('formats a fraction as a decimal', () => {
    expect(formatFramerate([30000, 1001])).toBe('29.97')
    expect(formatFramerate([24000, 1001])).toBe('23.976')
    expect(formatFramerate([30, 1])).toBe('30')
    expect(formatFramerate([100, 1])).toBe('100')
    expect(formatFramerate([25, 2])).toBe('12.5')
  })
  it('labels a stored value, or shows it as is', () => {
    expect(framerateLabel('30000/1001')).toBe('29.97')
    expect(framerateLabel('junk')).toBe('junk')
  })
  it('matches a preset by value', () => {
    expect(framerateChoice('29.97')).toBe('30000/1001')
    expect(framerateChoice('60/2')).toBe('30')
    expect(framerateChoice('31')).toBeUndefined()
    expect(framerateChoice('junk')).toBeUndefined()
  })
})

describe('unknownToken', () => {
  it('finds the first unknown token', () => {
    expect(unknownToken('{date}/{sourc}_{tim}.{ext}')).toBe('{sourc}')
    expect(unknownToken('{date}/{source}_{time}.{ext}')).toBeNull()
  })
  it('checks the innermost braces', () => {
    expect(unknownToken('{a{date}')).toBeNull()
    expect(unknownToken('{}')).toBe('{}')
  })
})

describe('splitTemplate and joinTemplate', () => {
  it('splits at the last slash', () => {
    expect(splitTemplate('~/rec/{date}/{source}.{ext}')).toEqual({ folder: '~/rec/{date}', name: '{source}', file: '{source}.{ext}' })
    expect(splitTemplate('/{source}.{ext}')).toEqual({ folder: '/', name: '{source}', file: '{source}.{ext}' })
    expect(splitTemplate('{source}.mov')).toEqual({ folder: '', name: null, file: '{source}.mov' })
  })
  it('joins with one slash', () => {
    expect(joinTemplate('~/rec//', '{source}')).toBe('~/rec/{source}.{ext}')
    expect(joinTemplate('/', '{source}')).toBe('/{source}.{ext}')
    expect(joinTemplate('', '{source}')).toBe('{source}.{ext}')
  })
  it('round-trips', () => {
    for (const t of ['~/rec/{date}/{source}.{ext}', '/{source}.{ext}', '{source}.{ext}']) {
      const { folder, name } = splitTemplate(t)
      expect(joinTemplate(folder, name!)).toBe(t)
    }
  })
  it('swaps a literal extension for the token', () => {
    expect(withExtToken('~/rec/{source}.mov')).toBe('~/rec/{source}.{ext}')
    expect(withExtToken('~/rec/{source}')).toBe('~/rec/{source}.{ext}')
    expect(withExtToken('~/rec/{source}.{ext}')).toBe('~/rec/{source}.{ext}')
  })
})

describe('templateRoot and applyPattern', () => {
  it('keeps the token-free leading folders', () => {
    expect(templateRoot('/Volumes/Media/{date}/{source}.{ext}')).toBe('/Volumes/Media')
    expect(templateRoot('~/rec/{source}.{ext}')).toBe('~/rec')
    expect(templateRoot('/{date}/{source}.{ext}')).toBe('/')
    expect(templateRoot('{date}/{source}.{ext}')).toBe('~/capture-room')
  })
  it('applies a pattern below the root', () => {
    expect(applyPattern('/Volumes/Media/{date}/{source}.{ext}', '{source_name}/{date}_{time}', false)).toBe(
      '/Volumes/Media/{source_name}/{date}_{time}.{ext}',
    )
    expect(applyPattern('/{date}/{source}.{ext}', '{date}/{source}_{time}', false)).toBe('/{date}/{source}_{time}.{ext}')
  })
  it('adds {output} when there are several outputs', () => {
    expect(applyPattern('~/rec/{source}.{ext}', '{date}/{source}_{time}', true)).toBe('~/rec/{date}/{source}_{time}_{output}.{ext}')
  })
})

describe('browserPlayable', () => {
  it('takes H.264 4:2:0 with AAC in .mp4 or .mov', () => {
    expect(browserPlayable(leg())).toBe(true)
    expect(browserPlayable(leg({ container: 'mov' }, { audio_codec: 'aac' }))).toBe(true)
  })
  it('rejects anything else', () => {
    expect(browserPlayable(leg({ codec: 'h265' }))).toBe(false)
    expect(browserPlayable(leg({ chroma: '422' }))).toBe(false)
    expect(browserPlayable(leg({ container: 'mkv' }))).toBe(false)
    expect(browserPlayable(leg({ container: 'mov' }, { audio_codec: 'pcm' }))).toBe(false)
  })
})

describe('expandPath', () => {
  const at = new Date(2026, 9, 5, 13, 10, 22)
  const vars = { source: 'cam1', sourceName: 'Cam: 1', node: 'studio', preset: '' }

  it('expands every token', () => {
    const l = leg({
      resolution: '1920x1080',
      framerate: '30000/1001',
      path_template:
        '~/{node}/{preset}/{date}/{source_name}_{time}_{datetime}_{year}{month}{day}_{output}_{codec}_{resolution}_{fps}_{take}.{ext}',
    })
    expect(expandPath(l, vars, at)).toBe('~/studio/preset/2026-10-05/Cam- 1_131022_20261005_131022_20261005_Main_h264_1920x1080_29.97_01.mp4')
  })
  it('says source for a matched resolution and frame rate', () => {
    expect(expandPath(leg({ path_template: '{resolution}_{fps}.{ext}' }), vars, at)).toBe('source_source.mp4')
  })
  it('makes values safe as one path component', () => {
    const names = { ...vars, source: 'a/b\\c', node: '..', preset: '  ' }
    expect(expandPath(leg({ name: '', path_template: '{source}+{node}+{preset}+{output}.{ext}' }), names, at)).toBe('a-b-c+_+_+_.mp4')
  })
  it('adds {segment} before the extension when splitting', () => {
    expect(expandPath(leg({}, { split_minutes: 15 }), vars, at)).toBe('~/rec/cam1_001.mp4')
    expect(expandPath(leg({ path_template: '~/rec.d/{source}' }, { split_gb: 4 }), vars, at)).toBe('~/rec.d/cam1_001')
    expect(expandPath(leg({ path_template: '~/rec/{segment}-{source}.{ext}' }, { split_gb: 4 }), vars, at)).toBe('~/rec/001-cam1.mp4')
  })
})

describe('legProblems', () => {
  it('passes a good leg', () => {
    expect(legProblems(leg({ resolution: '1920x1080', framerate: '29.97' }))).toEqual({})
  })
  it('flags bad sizes and frame rates', () => {
    expect(legProblems(leg({ resolution: '1920', framerate: 'fast' }))).toEqual({
      resolution: 'Enter a width and height',
      framerate: 'Use a number or a fraction, e.g. 29.97 or 30000/1001',
    })
  })
  it('ignores blank sizes and frame rates', () => {
    expect(legProblems(leg({ resolution: ' ', framerate: '' }))).toEqual({})
  })
  it('flags the path', () => {
    expect(legProblems(leg({ path_template: '  ' })).path).toBe('A path is required')
    expect(legProblems(leg({ path_template: '~/rec/.{ext}' })).path).toBe('A file name is required')
    expect(legProblems(leg({ path_template: '~/rec/{sorce}.{ext}' })).path).toBe(
      "{sorce} isn't a token — check the spelling, or pick one from the list",
    )
  })
})

describe('clashingLegs', () => {
  it('finds legs that would write the same file', () => {
    expect(clashingLegs([leg(), leg({ name: 'Backup' })])).toEqual(new Set([0, 1]))
    expect(clashingLegs([leg(), leg({ name: 'B', container: 'mov' }), leg({ name: 'C' })])).toEqual(new Set([0, 2]))
    expect(clashingLegs([leg(), leg(), leg()])).toEqual(new Set([0, 1, 2]))
  })
  it('tells legs apart by the tokens each settles', () => {
    const t = '~/rec/{source}_{output}.{ext}'
    expect(clashingLegs([leg({ path_template: t }), leg({ name: 'Backup', path_template: t })])).toEqual(new Set())
    const containers: Container[] = ['mp4', 'mov', 'mkv']
    expect(clashingLegs(containers.map((container) => leg({ container })))).toEqual(new Set())
  })
  it('compares split legs by their first file', () => {
    const t = '~/rec/{source}_001.{ext}'
    expect(clashingLegs([leg({}, { split_minutes: 15 }), leg({ path_template: t })])).toEqual(new Set([0, 1]))
  })
})
