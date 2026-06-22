<script setup lang="ts">
import { onMounted } from 'vue'
import { RouterLink, RouterView } from 'vue-router'
import { startWebSocket } from '@/composables/useWebSocket'
import {
  LayoutDashboard,
  Monitor,
  Video,
  Settings2,
  Server,
  CalendarClock,
  ScrollText,
} from '@lucide/vue'

onMounted(() => {
  startWebSocket()
})

const navItems = [
  { to: '/dashboard', label: 'Dashboard', icon: LayoutDashboard },
  { to: '/sources', label: 'Sources', icon: Monitor },
  { to: '/recordings', label: 'Recordings', icon: Video },
  { to: '/presets', label: 'Presets', icon: Settings2 },
  { to: '/nodes', label: 'Nodes', icon: Server },
  { to: '/schedules', label: 'Schedules', icon: CalendarClock },
  { to: '/logs', label: 'Logs', icon: ScrollText },
]
</script>

<template>
  <div class="flex h-svh bg-background text-foreground">
    <!-- Sidebar -->
    <aside class="w-56 shrink-0 border-r border-border flex flex-col">
      <div class="h-14 flex items-center px-4 border-b border-border">
        <span class="font-semibold tracking-tight">Capture Room</span>
      </div>
      <nav class="flex-1 overflow-y-auto py-2">
        <RouterLink
          v-for="item in navItems"
          :key="item.to"
          :to="item.to"
          class="flex items-center gap-3 px-4 py-2 text-sm rounded-md mx-2 transition-colors hover:bg-accent hover:text-accent-foreground"
          :class="{ 'bg-accent text-accent-foreground': $route.path.startsWith(item.to) }"
        >
          <component :is="item.icon" class="w-4 h-4 shrink-0" />
          {{ item.label }}
        </RouterLink>
      </nav>
    </aside>

    <!-- Main content -->
    <main class="flex-1 overflow-y-auto">
      <RouterView />
    </main>
  </div>
</template>
