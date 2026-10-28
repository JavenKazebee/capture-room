import { CalendarClock, Circle, Film, History, Monitor, MonitorPlay, ScrollText, Server, Settings, ShieldCheck, SlidersHorizontal, Wrench } from '@lucide/vue'

/** Top-level workspaces: operating desks plus Setup. */
export const workspaces = [
  { to: '/record', label: 'Record', icon: Circle },
  { to: '/recordings', label: 'Recordings', icon: Film },
  { to: '/playback', label: 'Playback', icon: MonitorPlay },
  { to: '/replay', label: 'Replay', icon: History },
  { to: '/schedule', label: 'Schedule', icon: CalendarClock },
  { to: '/setup', label: 'Setup', icon: Wrench },
]

/** Pages inside the Setup workspace. */
export const setupPages = [
  { to: '/setup/sources', label: 'Sources', icon: Monitor },
  { to: '/setup/presets', label: 'Presets', icon: SlidersHorizontal },
  { to: '/setup/nodes', label: 'Nodes', icon: Server },
  { to: '/setup/checks', label: 'Checks', icon: ShieldCheck },
  { to: '/setup/logs', label: 'Logs', icon: ScrollText },
  { to: '/setup/settings', label: 'Settings', icon: Settings },
]
