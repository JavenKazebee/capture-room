import { describe, expect, it } from 'vitest'
import { blankLeg } from '@/stores/presets'
import { expandSlateTitle } from '@/lib/codecs'
import { newSlate, slateProblem } from '@/lib/slate'

describe('slates', () => {
  it('checks title tokens and colours like the node', () => {
    expect(slateProblem(null)).toBeNull()
    expect(slateProblem({ ...newSlate('5s'), title: '{preset} take {take}' })).toBeNull()
    expect(slateProblem({ ...newSlate('5s'), title: '{source_name}.{ext}' })).toBe("{ext} isn't a slate title token")
    expect(slateProblem({ ...newSlate('5s'), title: '{sorce}' })).toBe("{sorce} isn't a slate title token")
    expect(slateProblem({ ...newSlate('5s'), text: 'white' })).toBe('Slate colours must be #rrggbb')
  })

  it('expands a title with names as they read', () => {
    const leg = { ...blankLeg(), name: 'Main' }
    const vars = { source: 's1', sourceName: 'Cam/1', node: 'n', preset: 'ProRes' }
    expect(expandSlateTitle('{preset}: {source_name} ({output})', leg, vars)).toBe('ProRes: Cam/1 (Main)')
  })
})
