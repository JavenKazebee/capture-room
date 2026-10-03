/**
 * A dense table with sortable columns, a text filter, per-browser column
 * visibility (see `ColumnsMenu`), and optional grouping. Cells render through
 * `#cell-<id>` slots, falling back to the column's `value`.
 */
export interface Column<R> {
  id: string
  label: string
  /** Sort key and fallback cell text. */
  value?: (row: R) => string | number | null | undefined
  /** Hidden until turned on in the columns menu. */
  hiddenByDefault?: boolean
  /** Can't be hidden (e.g. the name or actions). */
  alwaysVisible?: boolean
  align?: 'left' | 'right'
  class?: string
  headerClass?: string
}
