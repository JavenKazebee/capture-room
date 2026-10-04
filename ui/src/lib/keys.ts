/** Shortcut labels that match the viewer's platform: ⌘ on Apple devices, Ctrl elsewhere. */

const platform =
  (navigator as Navigator & { userAgentData?: { platform?: string } }).userAgentData?.platform ?? navigator.platform

export const isMac = /mac|iphone|ipad|ipod/i.test(platform)

/** The primary modifier's label: "⌘" or "Ctrl". */
export const modKey = isMac ? '⌘' : 'Ctrl'

/** A mod+key label, e.g. shortcut('K') → "⌘+K" on macOS, "Ctrl+K" elsewhere. */
export function shortcut(key: string) {
  return `${modKey}+${key}`
}
