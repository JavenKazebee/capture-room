import { Circle, Film, Monitor, MonitorPlay, Server, Settings, SlidersHorizontal, Wrench } from '@lucide/vue'

/**
 * Top-level workspaces: operating desks plus Setup. Replay joins here once
 * the backend supports it — not before.
 */
export const workspaces = [
  { to: '/record', label: 'Record', icon: Circle },
  { to: '/playback', label: 'Playback', icon: MonitorPlay },
  { to: '/recordings', label: 'Recordings', icon: Film },
  { to: '/setup', label: 'Setup', icon: Wrench },
]

/** Pages inside the Setup workspace. */
export const setupPages = [
  { to: '/setup/sources', label: 'Sources', icon: Monitor },
  { to: '/setup/presets', label: 'Presets', icon: SlidersHorizontal },
  { to: '/setup/nodes', label: 'Nodes', icon: Server },
  { to: '/setup/settings', label: 'Settings', icon: Settings },
]
