import { ListChecks, X } from '@lucide/vue'
import { registerCommands } from '@/composables/useCommands'
import type { useSelection } from '@/composables/useSelection'

/** "Select all <noun> shown" and "Clear selection" in the palette, for a table with checkboxes. */
export function useTableSelectionCommands(opts: {
  /** Prefix for the command ids, e.g. `sources`. */
  id: string
  /** What the rows are, e.g. `sources`. */
  noun: string
  selection: ReturnType<typeof useSelection>
  /** Keys of the rows the table shows after filtering. */
  shown: () => string[]
  enabled?: () => boolean
}) {
  const { id, noun, selection, shown, enabled = () => true } = opts
  registerCommands(() => {
    if (!enabled()) return []
    return [
      {
        id: `${id}:select-all`,
        title: `Select all ${noun} shown`,
        group: 'This page',
        icon: ListChecks,
        keywords: ['bulk', 'multiple', 'check'],
        run: () => selection.selectAll(shown()),
      },
      ...(selection.count.value
        ? [{ id: `${id}:clear-selection`, title: 'Clear selection', group: 'This page' as const, icon: X, keywords: ['deselect', 'none'], run: selection.clearSelection }]
        : []),
    ]
  })
}
