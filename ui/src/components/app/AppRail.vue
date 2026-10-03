<script setup lang="ts">
import { RouterLink, useRoute } from 'vue-router'
import { Search } from '@lucide/vue'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { navItems, settingsItem } from './navItems'

defineEmits<{ command: [] }>()
const route = useRoute()
</script>

<template>
  <nav class="w-12 shrink-0 flex flex-col items-center gap-1 py-2 bg-sidebar border-r border-sidebar-border">
    <RouterLink to="/multiview" class="mb-2 size-7 rounded-md bg-zinc-900 ring-1 ring-white/15 grid place-items-center" title="Capture Room">
      <span class="size-2.5 rounded-full bg-brand" />
    </RouterLink>

    <Tooltip v-for="item in navItems" :key="item.to">
      <TooltipTrigger as-child>
        <RouterLink
          :to="item.to"
          class="rail-item"
          :data-active="route.path.startsWith(item.to) || undefined"
        >
          <component :is="item.icon" class="size-[18px]" />
        </RouterLink>
      </TooltipTrigger>
      <TooltipContent side="right">{{ item.label }}</TooltipContent>
    </Tooltip>

    <div class="flex-1" />

    <Tooltip>
      <TooltipTrigger as-child>
        <button class="rail-item" @click="$emit('command')">
          <Search class="size-[18px]" />
        </button>
      </TooltipTrigger>
      <TooltipContent side="right">Command palette <span class="opacity-60 ml-1">Ctrl K</span></TooltipContent>
    </Tooltip>
    <Tooltip>
      <TooltipTrigger as-child>
        <RouterLink
          :to="settingsItem.to"
          class="rail-item"
          :data-active="route.path.startsWith(settingsItem.to) || undefined"
        >
          <component :is="settingsItem.icon" class="size-[18px]" />
        </RouterLink>
      </TooltipTrigger>
      <TooltipContent side="right">{{ settingsItem.label }}</TooltipContent>
    </Tooltip>
  </nav>
</template>

<style scoped>
@reference "@/style.css";
.rail-item {
  @apply relative size-9 grid place-items-center rounded-md text-muted-foreground transition-colors hover:text-foreground hover:bg-sidebar-accent focus-visible:outline-2 focus-visible:outline-ring;
}
.rail-item[data-active] {
  @apply text-foreground bg-sidebar-accent;
}
.rail-item[data-active]::before {
  content: '';
  @apply absolute -left-1.5 inset-y-2 w-0.5 rounded-full bg-primary;
}
</style>
