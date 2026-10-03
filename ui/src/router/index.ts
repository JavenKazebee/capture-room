import { createRouter, createWebHistory } from 'vue-router'

export const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', redirect: '/record' },
    {
      path: '/record',
      name: 'record',
      component: () => import('@/views/RecordView.vue'),
    },
    {
      path: '/setup',
      component: () => import('@/views/SetupLayout.vue'),
      redirect: '/setup/sources',
      children: [
        { path: 'sources', name: 'sources', component: () => import('@/views/SourcesView.vue') },
        { path: 'presets', name: 'presets', component: () => import('@/views/PresetsView.vue') },
        { path: 'nodes', name: 'nodes', component: () => import('@/views/NodesView.vue') },
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
