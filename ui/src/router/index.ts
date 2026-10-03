import { createRouter, createWebHistory } from 'vue-router'

export const router = createRouter({
  history: createWebHistory(),
  routes: [
    {
      path: '/',
      redirect: '/dashboard',
    },
    {
      path: '/dashboard',
      name: 'dashboard',
      component: () => import('@/views/DashboardView.vue'),
    },
    {
      path: '/sources',
      name: 'sources',
      component: () => import('@/views/SourcesView.vue'),
    },
    {
      path: '/presets',
      name: 'presets',
      component: () => import('@/views/PresetsView.vue'),
    },
    {
      path: '/nodes',
      name: 'nodes',
      component: () => import('@/views/NodesView.vue'),
    },
  ],
})
