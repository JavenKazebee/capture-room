<script setup lang="ts">
import { RouterLink, useRoute } from 'vue-router'
import { useNodesStore } from '@/stores/nodes'
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarRail,
} from '@/components/ui/sidebar'
import { navItems } from './navItems'

const route = useRoute()
const nodes = useNodesStore()
</script>

<template>
  <Sidebar collapsible="icon">
    <SidebarHeader class="h-11 justify-center border-b border-sidebar-border">
      <div class="flex items-center gap-2 px-1 overflow-hidden">
        <span class="size-5 shrink-0 rounded-[5px] bg-zinc-900 ring-1 ring-white/15 grid place-items-center">
          <span class="size-2 rounded-full bg-primary" />
        </span>
        <span class="font-semibold tracking-tight truncate group-data-[collapsible=icon]:hidden">
          Capture Room
        </span>
      </div>
    </SidebarHeader>

    <SidebarContent>
      <SidebarGroup>
        <SidebarMenu>
          <SidebarMenuItem v-for="item in navItems" :key="item.to">
            <SidebarMenuButton
              as-child
              :tooltip="item.label"
              :is-active="route.path.startsWith(item.to)"
              class="relative data-[active=true]:before:absolute data-[active=true]:before:-left-2 data-[active=true]:before:inset-y-1.5 data-[active=true]:before:w-0.5 data-[active=true]:before:rounded-full data-[active=true]:before:bg-primary"
            >
              <RouterLink :to="item.to">
                <component :is="item.icon" />
                <span>{{ item.label }}</span>
              </RouterLink>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarGroup>
    </SidebarContent>

    <SidebarFooter class="text-[11px] text-muted-foreground group-data-[collapsible=icon]:hidden">
      <div v-if="nodes.self" class="px-2 leading-tight">
        <div class="truncate text-sidebar-foreground">{{ nodes.self.node_name }}</div>
        <div class="num truncate">
          {{ nodes.self.node_id.slice(0, 8) }}
          <template v-if="nodes.nodes.find((n) => n.is_self)?.version">
            · v{{ nodes.nodes.find((n) => n.is_self)?.version }}
          </template>
        </div>
      </div>
    </SidebarFooter>
    <SidebarRail />
  </Sidebar>
</template>
