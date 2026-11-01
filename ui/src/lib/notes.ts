import { h, type VNode } from 'vue'

/**
 * Release highlights: the little markdown a release body's top uses (paragraphs, `-`/`*`
 * bullets, `**bold**`, `` `code` ``, `[text](https://…)` links), built as VNodes so Vue escapes
 * every bit of text and nothing goes through `v-html`. Anything else shows as written.
 */
export type Inline =
  | { kind: 'text'; text: string }
  | { kind: 'code'; text: string }
  | { kind: 'bold'; children: Inline[] }
  | { kind: 'link'; text: string; href: string }

export type Block = { kind: 'p'; text: Inline[] } | { kind: 'ul'; items: Inline[][] }

const BULLET = /^\s*[-*]\s+(.*)$/
// Only https links: a `javascript:` one would run on click.
const INLINE = /`([^`]+)`|\*\*(.+?)\*\*|\[([^\]]+)\]\((https:\/\/[^\s)]+)\)/g

export function parseInline(text: string, bold = true): Inline[] {
  const out: Inline[] = []
  let last = 0
  for (const m of text.matchAll(INLINE)) {
    const [whole, code, strong, label, href] = m
    if (strong !== undefined && !bold) continue
    if (m.index > last) out.push({ kind: 'text', text: text.slice(last, m.index) })
    if (code !== undefined) out.push({ kind: 'code', text: code })
    else if (strong !== undefined) out.push({ kind: 'bold', children: parseInline(strong, false) })
    else out.push({ kind: 'link', text: label!, href: href! })
    last = m.index + whole.length
  }
  if (last < text.length) out.push({ kind: 'text', text: text.slice(last) })
  return out
}

export function parseNotes(markdown: string): Block[] {
  const blocks: Block[] = []
  // Lines of the paragraph or bullet being read; a bullet's wrapped lines join it.
  let para: string[] | null = null
  let list: string[][] | null = null
  const flush = () => {
    if (para) blocks.push({ kind: 'p', text: parseInline(para.join(' ')) })
    if (list) blocks.push({ kind: 'ul', items: list.map((item) => parseInline(item.join(' '))) })
    para = list = null
  }
  for (const raw of markdown.split(/\r?\n/)) {
    const line = raw.trim()
    const bullet = BULLET.exec(raw)
    if (!line) {
      flush()
    } else if (bullet) {
      if (para) flush()
      list ??= []
      list.push([bullet[1]!.trim()])
    } else if (list) {
      list[list.length - 1]!.push(line)
    } else {
      ;(para ??= []).push(line)
    }
  }
  flush()
  return blocks
}

function renderInline(parts: Inline[]): (VNode | string)[] {
  return parts.map((p) => {
    switch (p.kind) {
      case 'text':
        return p.text
      case 'code':
        return h('code', { class: 'rounded bg-muted px-1 py-0.5 font-mono text-[0.9em]' }, p.text)
      case 'bold':
        return h('strong', { class: 'font-semibold' }, renderInline(p.children))
      case 'link':
        return h('a', { href: p.href, target: '_blank', rel: 'noopener noreferrer', class: 'underline underline-offset-2' }, p.text)
    }
  })
}

export function renderNotes(markdown: string): VNode[] {
  return parseNotes(markdown).map((b) =>
    b.kind === 'p'
      ? h('p', renderInline(b.text))
      : h(
          'ul',
          { class: 'list-disc space-y-1 pl-4' },
          b.items.map((item) => h('li', renderInline(item))),
        ),
  )
}
