import { PATH_TOKEN_GROUPS, unknownToken } from '@/lib/codecs'
import type { SlateLength } from '@/types/generated/SlateLength'
import type { SlateSettings } from '@/types/generated/SlateSettings'

/** A recording's slate lengths; playlist items leave out `frame`. */
export const SLATE_LENGTHS: { value: SlateLength; label: string }[] = [
  { value: 'frame', label: '1 frame' },
  { value: '3s', label: '3 s' },
  { value: '5s', label: '5 s' },
  { value: '10s', label: '10 s' },
]

/** Path tokens without the file name parts, as `unknown_title_token` in `profile.rs` allows. */
export const SLATE_TOKEN_GROUPS = PATH_TOKEN_GROUPS.map((g) => ({
  label: g.label,
  tokens: g.tokens.filter((t) => t.token !== '{ext}' && t.token !== '{segment}'),
})).filter((g) => g.tokens.length)

export function newSlate(length: SlateLength): SlateSettings {
  return { length, title: '', background: '#1a1a1a', text: '#ffffff' }
}

const COLOUR = /^#[0-9a-fA-F]{6}$/

/** What's wrong with a slate's settings; mirrors `slate::check` on the node. */
export function slateProblem(s: SlateSettings | null): string | null {
  if (!s) return null
  const bad = unknownToken(s.title) ?? (/\{(ext|segment)\}/.exec(s.title)?.[0] ?? null)
  if (bad) return `${bad} isn't a slate title token`
  if (!COLOUR.test(s.background) || !COLOUR.test(s.text)) return 'Slate colours must be #rrggbb'
  return null
}
