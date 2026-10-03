import { Circle, Monitor, Server, Settings, SlidersHorizontal, Wrench } from '@lucide/vue'

/**
 * Top-level workspaces: operating desks plus Setup. Replay and Playback join
 * here once the backend supports them — not before.
 */
export const workspaces = [
  { to: '/record', label: 'Record', icon: Circle },
  { to: '/setup', label: 'Setup', icon: Wrench },
]

/** Pages inside the Setup workspace. */
export const setupPages = [
  { to: '/setup/sources', label: 'Sources', icon: Monitor },
  { to: '/setup/presets', label: 'Presets', icon: SlidersHorizontal },
  { to: '/setup/nodes', label: 'Nodes', icon: Server },
  { to: '/setup/settings', label: 'Settings', icon: Settings },
]
