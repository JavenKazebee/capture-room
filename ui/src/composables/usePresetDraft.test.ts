import { describe, expect, it } from 'vitest'
import { withOutputToken } from './usePresetDraft'

describe('withOutputToken', () => {
  it('puts {output} at the start of the file name', () => {
    expect(withOutputToken('~/capture-room/{date}/{source_name}_{datetime}.{ext}')).toBe(
      '~/capture-room/{date}/{output}_{source_name}_{datetime}.{ext}',
    )
    expect(withOutputToken('D:\\rec\\{source_name}.{ext}')).toBe('D:\\rec\\{output}_{source_name}.{ext}')
    expect(withOutputToken('{source_name}.{ext}')).toBe('{output}_{source_name}.{ext}')
  })

  it('leaves a template that already has {output}', () => {
    expect(withOutputToken('~/r/{output}/{source_name}.{ext}')).toBe('~/r/{output}/{source_name}.{ext}')
  })
})
