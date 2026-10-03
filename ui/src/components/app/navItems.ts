import { LayoutGrid, Monitor, Server, Settings, SlidersHorizontal } from '@lucide/vue'

export const navItems = [
  { to: '/multiview', label: 'Multiview', icon: LayoutGrid },
  { to: '/sources', label: 'Sources', icon: Monitor },
  { to: '/presets', label: 'Presets', icon: SlidersHorizontal },
  { to: '/nodes', label: 'Nodes', icon: Server },
]

export const settingsItem = { to: '/settings', label: 'Settings', icon: Settings }
