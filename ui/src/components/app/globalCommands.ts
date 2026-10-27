import { useRouter } from 'vue-router'
import {
  CalendarClock,
  CalendarOff,
  Circle,
  Clock,
  Download,
  Film,
  Gauge,
  Info,
  KeyRound,
  LogOut,
  MonitorCog,
  MoonStar,
  Palette,
  Pencil,
  Radar,
  Rows3,
  ScanEye,
  Square,
  Sun,
} from '@lucide/vue'
import { registerCommands, type Command } from '@/composables/useCommands'
import { usePreferences, type Density } from '@/composables/usePreferences'
import { authStatus, signOut } from '@/composables/useAuth'
import { dateTime } from '@/lib/format'
import { scheduleInput } from '@/lib/schedules'
import { shortcut } from '@/lib/keys'
import { notifyError } from '@/lib/notify'
import { useNodesStore } from '@/stores/nodes'
import { useRecordDeskStore } from '@/stores/recordDesk'
import { useRecordingsStore } from '@/stores/recordings'
import { useSchedulesStore } from '@/stores/schedules'
import { useSourcesStore, type Source } from '@/stores/sources'
import { scrollToHash } from '@/router'
import { setupPages, workspaces } from './workspaces'

/** Every page: the operating workspaces, then Setup's pages in place of Setup. */
const destinations = [...workspaces.filter((w) => w.to !== '/setup'), ...setupPages]
const SHORTCUTS: Record<string, string> = { '/setup/logs': shortcut('J') }

const SETTINGS_SECTIONS = [
  { hash: 'appearance', label: 'Appearance', icon: Palette, keywords: ['theme', 'density', 'dark', 'light'] },
  { hash: 'monitoring', label: 'Monitoring', icon: Gauge, keywords: ['thumbnail', 'meter', 'levels'] },
  { hash: 'clock-sync', label: 'Clock sync', icon: Clock, keywords: ['ptp', 'ntp', 'time'] },
  { hash: 'access-token', label: 'Access token', icon: KeyRound, keywords: ['password', 'sign in', 'auth', 'security'] },
  { hash: 'updates', label: 'Updates', icon: Download, keywords: ['pre-release', 'beta', 'release candidate', 'channel', 'version'] },
  { hash: 'about', label: 'About', icon: Info, keywords: ['version', 'uptime', 'fingerprint'] },
]

const DENSITIES: Density[] = ['compact', 'default', 'comfortable']

/** At most this many recordings are searchable, newest first. */
const MAX_RECORDINGS = 300

/** The palette's app-wide commands; pages add their own with `registerCommands`. */
export function useGlobalCommands() {
  const router = useRouter()
  const sources = useSourcesStore()
  const recordings = useRecordingsStore()
  const schedules = useSchedulesStore()
  const desk = useRecordDeskStore()
  const nodes = useNodesStore()
  const { colorMode, density } = usePreferences()

  const multiNode = () => nodes.nodes.length > 1
  const sourceDetail = (s: Source) => (multiNode() ? nodes.nameOf(s.node_id) : undefined)

  registerCommands(() =>
    destinations.map((d) => ({
      id: `go:${d.to}`,
      title: d.label,
      group: 'Go to',
      icon: d.icon,
      keywords: ['go', 'open', 'page'],
      shortcut: SHORTCUTS[d.to],
      run: () => router.push(d.to),
    })),
  )

  // Record or stop each source, as the Record desk would (its preset, its capacity checks).
  registerCommands(() =>
    sources.sources.map((s): Command => {
      const live = !!recordings.activeForSource(s.node_id, s.id)
      const preset = desk.presetOptions.find((o) => o.value === desk.presetIdOf(s.key))?.label
      return {
        id: `record:${s.key}`,
        title: `${live ? 'Stop' : 'Record'} ${s.display_name}`,
        detail: [sourceDetail(s), live ? 'recording' : preset].filter(Boolean).join(' · '),
        group: 'Recording',
        icon: live ? Square : Circle,
        iconClass: 'text-tally!',
        keywords: live ? ['stop', 'end'] : ['record', 'start', 'rec'],
        run: () => desk.toggle(s),
      }
    }),
  )

  registerCommands(() =>
    sources.sources.flatMap((s): Command[] => [
      {
        id: `source:show:${s.key}`,
        title: `Show ${s.display_name} in Record`,
        detail: sourceDetail(s),
        group: 'Sources',
        icon: ScanEye,
        keywords: ['source', 'feed', 'tile', 'select', s.id, s.source_type],
        searchOnly: true,
        run: () => {
          if (!desk.visible.some((v) => v.key === s.key)) desk.clearFilters()
          desk.select(s.key)
          return router.push('/record')
        },
      },
      {
        id: `source:edit:${s.key}`,
        title: `Edit ${s.display_name}`,
        detail: sourceDetail(s),
        group: 'Sources',
        icon: Pencil,
        keywords: ['source', 'settings', 'configure', s.id, s.source_type],
        searchOnly: true,
        run: () => router.push({ path: '/setup/sources', query: { edit: s.key } }),
      },
    ]),
  )

  registerCommands(() =>
    recordings.history.slice(0, MAX_RECORDINGS).map((r) => ({
      id: `recording:${r.node_id}/${r.id}`,
      title: recordings.sourceNameOf(r),
      detail: [dateTime(r.started_at), multiNode() ? nodes.nameOf(r.node_id) : null].filter(Boolean).join(' · '),
      group: 'Recordings',
      icon: Film,
      keywords: ['recording', 'take', recordings.presetNameOf(r), r.status, r.id],
      searchOnly: true,
      bias: -250,
      run: () => router.push({ path: '/recordings', query: { select: `${r.node_id}/${r.id}` } }),
    })),
  )

  registerCommands(() =>
    !nodes.isController
      ? []
      : schedules.schedules.flatMap((s): Command[] => {
          const running = s.last_run?.status === 'running'
          const out: Command[] = [
            {
              id: `schedule:open:${s.id}`,
              title: s.name,
              detail: running ? 'running' : s.enabled ? undefined : 'disabled',
              group: 'Schedules',
              icon: CalendarClock,
              keywords: ['schedule', 'open'],
              searchOnly: true,
              run: () => router.push({ path: '/schedule', query: { select: s.id } }),
            },
            {
              id: `schedule:edit:${s.id}`,
              title: `Edit ${s.name}`,
              group: 'Schedules',
              icon: Pencil,
              keywords: ['schedule', 'change'],
              searchOnly: true,
              run: () => router.push({ path: '/schedule', query: { edit: s.id } }),
            },
            {
              id: `schedule:enable:${s.id}`,
              title: `${s.enabled ? 'Disable' : 'Enable'} ${s.name}`,
              group: 'Schedules',
              icon: s.enabled ? CalendarOff : CalendarClock,
              keywords: ['schedule', s.enabled ? 'turn off pause' : 'turn on resume'],
              searchOnly: true,
              run: () =>
                schedules
                  .update(s.id, { ...scheduleInput(s), enabled: !s.enabled })
                  .catch((e) => notifyError(`Couldn’t ${s.enabled ? 'disable' : 'enable'} “${s.name}”`, e)),
            },
          ]
          // A running schedule's Stop is listed without searching, like Record/Stop.
          if (running) {
            out.unshift({
              id: `schedule:stop:${s.id}`,
              title: `Stop ${s.name} now`,
              detail: 'running',
              group: 'Schedules',
              icon: Square,
              iconClass: 'text-tally!',
              keywords: ['schedule', 'stop', 'end'],
              run: () => schedules.stop(s.id).catch((e) => notifyError(`Couldn’t stop “${s.name}”`, e)),
            })
          }
          return out
        }),
  )

  registerCommands(() =>
    SETTINGS_SECTIONS.filter((s) => s.hash !== 'clock-sync' || nodes.isController).map((s) => ({
      id: `settings:${s.hash}`,
      title: `Settings: ${s.label}`,
      group: 'Settings',
      icon: s.icon,
      keywords: ['settings', 'preferences', ...s.keywords],
      run: () => {
        const to = { path: '/setup/settings', hash: `#${s.hash}` }
        // Pushing the URL already shown is a no-op, so scroll ourselves.
        if (router.currentRoute.value.fullPath === router.resolve(to).fullPath) scrollToHash(to.hash)
        else router.push(to)
      },
    })),
  )

  registerCommands(() => [
    {
      id: 'scan',
      title: 'Scan for sources',
      group: 'Commands',
      icon: Radar,
      keywords: ['ndi', 'discover', 'find', 'devices'],
      run: () => sources.scanWithToast(),
    },
    {
      id: 'theme',
      title: `Switch to ${colorMode.value === 'dark' ? 'light' : 'dark'} theme`,
      group: 'Commands',
      icon: colorMode.value === 'dark' ? Sun : MoonStar,
      keywords: ['theme', 'dark', 'light', 'appearance', 'mode'],
      run: () => (colorMode.value = colorMode.value === 'dark' ? 'light' : 'dark'),
    },
    {
      id: 'theme:system',
      title: 'Follow the system theme',
      detail: colorMode.value === 'auto' ? 'current' : undefined,
      group: 'Commands',
      icon: MonitorCog,
      keywords: ['theme', 'auto', 'system', 'appearance'],
      searchOnly: true,
      run: () => (colorMode.value = 'auto'),
    },
    ...DENSITIES.map((d) => ({
      id: `density:${d}`,
      title: `Density: ${d}`,
      detail: density.value === d ? 'current' : undefined,
      group: 'Commands' as const,
      icon: Rows3,
      keywords: ['density', 'spacing', 'size', 'zoom'],
      run: () => (density.value = d),
    })),
    ...(authStatus.value?.required && authStatus.value.signed_in
      ? [
          {
            id: 'sign-out',
            title: 'Sign out',
            group: 'Commands' as const,
            icon: LogOut,
            keywords: ['log out', 'token', 'auth'],
            searchOnly: true,
            run: () => signOut(),
          },
        ]
      : []),
  ])
}
