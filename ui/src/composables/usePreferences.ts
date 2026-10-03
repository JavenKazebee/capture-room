import { useColorMode, useStorage } from '@vueuse/core'
import { watchEffect } from 'vue'

export type Density = 'compact' | 'default' | 'comfortable'

/** Overlays drawn on Multiview tiles. */
export interface TileOverlays {
  timecode: boolean
  meters: boolean
  node: boolean
  format: boolean
}

/**
 * Per-viewer UI preferences, kept in localStorage. Each is surfaced in the
 * view it affects (and on the Settings page) rather than hidden away.
 */
const colorMode = useColorMode({ initialValue: 'dark', storageKey: 'cr.theme' })
const density = useStorage<Density>('cr.density', 'default')
const tileSize = useStorage('cr.multiview.tileSize', 320)
const overlays = useStorage<TileOverlays>(
  'cr.multiview.overlays',
  { timecode: true, meters: true, node: true, format: false },
  undefined,
  { mergeDefaults: true },
)
const inspectorOpen = useStorage('cr.multiview.inspector', true)
/** Column visibility per table, keyed by table id. */
const columns = useStorage<Record<string, Record<string, boolean>>>('cr.columns', {})

watchEffect(() => {
  const root = document.documentElement
  if (density.value === 'default') delete root.dataset.density
  else root.dataset.density = density.value
})

export function usePreferences() {
  return { colorMode, density, tileSize, overlays, inspectorOpen, columns }
}
