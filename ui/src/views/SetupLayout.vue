<script setup lang="ts">
import { RouterLink, RouterView } from 'vue-router'
import { setupPages } from '@/components/app/workspaces'
</script>

<template>
  <div class="h-full flex min-h-0">
    <nav class="w-44 shrink-0 border-r border-border bg-sidebar p-2 space-y-0.5 overflow-y-auto">
      <RouterLink
        v-for="p in setupPages"
        :key="p.to"
        :to="p.to"
        class="flex items-center gap-2.5 h-8 px-2.5 rounded-md text-sm text-muted-foreground hover:text-foreground hover:bg-sidebar-accent"
        active-class="!text-foreground bg-sidebar-accent font-medium"
      >
        <component :is="p.icon" class="size-4" />
        {{ p.label }}
      </RouterLink>
    </nav>
    <div class="flex-1 min-w-0 overflow-y-auto">
      <RouterView />
    </div>
  </div>
</template>
