import { describe, expect, it } from 'vitest'
import { parseInline, parseNotes } from './notes'

describe('parseNotes', () => {
  it('splits paragraphs on blank lines and joins wrapped lines', () => {
    expect(parseNotes('One\nstill one\r\n\r\nTwo')).toEqual([
      { kind: 'p', text: [{ kind: 'text', text: 'One still one' }] },
      { kind: 'p', text: [{ kind: 'text', text: 'Two' }] },
    ])
  })

  it('reads - and * bullets, with wrapped lines', () => {
    expect(parseNotes('Intro\n- a\n  more\n* b')).toEqual([
      { kind: 'p', text: [{ kind: 'text', text: 'Intro' }] },
      {
        kind: 'ul',
        items: [[{ kind: 'text', text: 'a more' }], [{ kind: 'text', text: 'b' }]],
      },
    ])
  })

  it('is empty for blank notes', () => {
    expect(parseNotes('  \n\n')).toEqual([])
  })
})

describe('parseInline', () => {
  it('reads bold, code and links', () => {
    expect(parseInline('**Replay** in `mp4`, see [docs](https://example.com/a).')).toEqual([
      { kind: 'bold', children: [{ kind: 'text', text: 'Replay' }] },
      { kind: 'text', text: ' in ' },
      { kind: 'code', text: 'mp4' },
      { kind: 'text', text: ', see ' },
      { kind: 'link', text: 'docs', href: 'https://example.com/a' },
      { kind: 'text', text: '.' },
    ])
  })

  it('keeps links that are not https as text', () => {
    expect(parseInline('[x](javascript:alert(1))')).toEqual([{ kind: 'text', text: '[x](javascript:alert(1))' }])
  })

  it('keeps markup it does not know as text', () => {
    expect(parseInline('<b>hi</b> _x_')).toEqual([{ kind: 'text', text: '<b>hi</b> _x_' }])
  })
})
