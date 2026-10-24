import { createRouter, createWebHistory } from 'vue-router'

/**
 * Wait for `#id` to render and stop moving, then scroll it into view. The
 * target's page loads lazily and fills in sections above it as data arrives,
 * so scrolling at once would land short. Pages scroll inside their own
 * container rather than the window, so this scrolls the element itself
 * instead of returning a position for the router to apply.
 */
export function scrollToHash(hash: string) {
  const id = decodeURIComponent(hash.slice(1))
  const deadline = performance.now() + 3000
  let lastTop: number | null = null
  let still = 0
  const step = () => {
    const el = document.getElementById(id)
    const top = el?.getBoundingClientRect().top ?? null
    still = el && top === lastTop ? still + 1 : 0
    lastTop = top
    // A few frames in the same place: the page above it has settled.
    if (el && (still >= 5 || performance.now() > deadline)) {
      el.scrollIntoView({ behavior: 'smooth', block: 'start' })
    } else if (performance.now() <= deadline) {
      requestAnimationFrame(step)
    }
  }
  requestAnimationFrame(step)
}

export const router = createRouter({
  history: createWebHistory(),
  scrollBehavior(to, _from, savedPosition) {
    if (to.hash) {
      scrollToHash(to.hash)
      return false
    }
    return savedPosition ?? false
  },
  routes: [
    { path: '/', redirect: '/record' },
    {
      path: '/record',
      name: 'record',
      component: () => import('@/views/RecordView.vue'),
    },
    {
      path: '/playback',
      name: 'playback',
      component: () => import('@/views/PlaybackView.vue'),
    },
    {
      path: '/replay',
      name: 'replay',
      component: () => import('@/views/ReplayView.vue'),
    },
    {
      path: '/recordings',
      name: 'recordings',
      component: () => import('@/views/RecordingsView.vue'),
    },
    {
      path: '/schedule',
      name: 'schedule',
      component: () => import('@/views/SchedulesView.vue'),
    },
    {
      path: '/setup',
      component: () => import('@/views/SetupLayout.vue'),
      redirect: '/setup/sources',
      children: [
        { path: 'sources', name: 'sources', component: () => import('@/views/SourcesView.vue') },
        { path: 'presets', name: 'presets', component: () => import('@/views/PresetsView.vue') },
        { path: 'nodes', name: 'nodes', component: () => import('@/views/NodesView.vue') },
        { path: 'checks', name: 'checks', component: () => import('@/views/ChecksView.vue') },
        { path: 'logs', name: 'logs', component: () => import('@/views/LogsView.vue') },
        { path: 'settings', name: 'settings', component: () => import('@/views/SettingsView.vue') },
      ],
    },
    // Old paths, kept working for bookmarks.
    { path: '/dashboard', redirect: '/record' },
    { path: '/multiview', redirect: '/record' },
    { path: '/sources', redirect: '/setup/sources' },
    { path: '/presets', redirect: '/setup/presets' },
    { path: '/nodes', redirect: '/setup/nodes' },
    { path: '/settings', redirect: '/setup/settings' },
  ],
})
