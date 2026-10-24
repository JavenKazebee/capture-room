import { computed, getCurrentScope, onScopeDispose, shallowReactive, type Component } from 'vue'
import { useStorage } from '@vueuse/core'

/** A command palette entry. */
export interface Command {
  /** Stable across renders and reloads: recents are stored by it. */
  id: string
  title: string
  /** Shown dimmed after the title, e.g. the node or the preset. */
  detail?: string
  group: CommandGroup
  icon?: Component
  /** Extra words it matches, beyond the title. */
  keywords?: string[]
  /** A key hint shown on the right, e.g. "⌘+J". */
  shortcut?: string
  /** Classes for the icon, e.g. the tally red for Record. */
  iconClass?: string
  /** Only listed once something is typed: one per recording, schedule, … */
  searchOnly?: boolean
  /** Added to its match score: negative for bulk entries (old recordings) that shouldn't outrank actions. */
  bias?: number
  run: () => unknown
}

/** Groups in the order the palette lists them. */
export const GROUPS = [
  'Go to',
  'This page',
  'Recording',
  'Schedules',
  'Recordings',
  'Sources',
  'Settings',
  'Commands',
] as const
export type CommandGroup = (typeof GROUPS)[number]

const providers = shallowReactive(new Map<symbol, () => Command[]>())

/**
 * Add commands to the palette. `fn` is re-run when what it reads changes; the
 * commands go away when the calling component (or effect scope) does.
 */
export function registerCommands(fn: () => Command[]) {
  const key = Symbol()
  providers.set(key, fn)
  const unregister = () => providers.delete(key)
  if (getCurrentScope()) onScopeDispose(unregister)
  return unregister
}

/** Every registered command; the first registration wins a duplicate id. */
export const allCommands = computed(() => {
  const seen = new Set<string>()
  const out: Command[] = []
  for (const fn of providers.values()) {
    for (const c of fn()) {
      if (seen.has(c.id)) continue
      seen.add(c.id)
      out.push(c)
    }
  }
  return out
})

const MAX_RECENT = 8
/** Ids of the commands run last, newest first. */
export const recentCommandIds = useStorage<string[]>('cr.commands.recent', [])

export function noteCommandRun(id: string) {
  recentCommandIds.value = [id, ...recentCommandIds.value.filter((x) => x !== id)].slice(0, MAX_RECENT)
}

// ── Fuzzy matching ───────────────────────────────────────────────────────────

const WORD_BREAK = /[\s\-_/.:›·—(]/

/**
 * How well `query` (one lowercase word) matches `text`: 0 for no match. A prefix
 * beats a word start, which beats a substring, which beats letters in order.
 */
function scoreWord(query: string, text: string): number {
  const t = text.toLowerCase()
  const at = t.indexOf(query)
  if (at === 0) return 1000 - t.length
  if (at > 0) return (WORD_BREAK.test(t[at - 1]!) ? 800 : 500) - at - t.length * 0.1

  // Letters in order: reward runs and word starts, penalise gaps.
  let score = 0
  let ti = 0
  let prev = -2
  for (const ch of query) {
    const found = t.indexOf(ch, ti)
    if (found === -1) return 0
    if (found === prev + 1) score += 8
    else if (found === 0 || WORD_BREAK.test(t[found - 1]!)) score += 6
    else score += 1 - Math.min(found - ti, 10) * 0.1
    prev = found
    ti = found + 1
  }
  return Math.max(1, Math.min(score * 4, 400) - t.length * 0.1)
}

/** A command's score for a whole query: every word must match its title, keywords or group. */
export function scoreCommand(query: string, c: Command): number {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean)
  if (!words.length) return 1
  const fields: [string, number][] = [
    [c.title, 1],
    ...(c.detail ? [[c.detail, 0.7] as [string, number]] : []),
    ...(c.keywords ?? []).map((k) => [k, 0.8] as [string, number]),
    [c.group, 0.5],
  ]
  let total = 0
  for (const w of words) {
    let best = 0
    for (const [text, weight] of fields) best = Math.max(best, scoreWord(w, text) * weight)
    if (best === 0) return 0
    total += best
  }
  return Math.max(1, total + (c.bias ?? 0))
}
