import { useEventListener } from '@vueuse/core'

/** Where a page's single-key shortcuts never apply: typing, and dialogs and menus over the page. */
const IGNORED = 'input, textarea, select, [role="dialog"], [role="menu"]'

/**
 * A page's keyboard shortcuts. Keys typed into a field, dialog or menu are
 * left alone, and so (unless `modifiers`) are Ctrl, ⌘ and Alt combinations.
 * `alsoIgnore` adds elements where the page's keys mean something else.
 */
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
