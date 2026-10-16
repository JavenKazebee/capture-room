import { useEventListener } from '@vueuse/core'

/** Where a page's single-key shortcuts never apply: typing, and dialogs and menus over the page. */
const IGNORED = 'input, textarea, select, [role="dialog"], [role="menu"]'

/** A page's shortcuts. Ctrl/⌘/Alt combinations are ignored unless `modifiers`; `alsoIgnore` adds elements. */
export function useViewKeys(
  handler: (e: KeyboardEvent) => void,
  opts: { alsoIgnore?: string; modifiers?: boolean } = {},
) {
  const ignored = opts.alsoIgnore ? `${IGNORED}, ${opts.alsoIgnore}` : IGNORED
  useEventListener('keydown', (e: KeyboardEvent) => {
    if ((e.target as HTMLElement).closest(ignored)) return
    if (!opts.modifiers && (e.ctrlKey || e.metaKey || e.altKey)) return
    handler(e)
  })
}
