<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { RouterView } from 'vue-router'
import { useIntervalFn } from '@vueuse/core'
import { reloadAll, startWebSocket } from '@/composables/useWebSocket'
import { usePreferences } from '@/composables/usePreferences'
import { usePresetsStore } from '@/stores/presets'
import { useStorageStore } from '@/stores/storage'
import { SidebarInset, SidebarProvider } from '@/components/ui/sidebar'
import { Toaster } from '@/components/ui/sonner'
import AppSidebar from '@/components/app/AppSidebar.vue'
import StatusBar from '@/components/app/StatusBar.vue'
import CommandPalette from '@/components/app/CommandPalette.vue'

const { colorMode } = usePreferences()
const storage = useStorageStore()
const commandOpen = ref(false)

onMounted(async () => {
  startWebSocket()
  usePresetsStore().load()
  await reloadAll()
  storage.load()
})

// Free space changes slowly and isn't evented; poll it for the status bar.
useIntervalFn(() => storage.load(), 30_000)
</script>

<template>
  <SidebarProvider class="h-svh min-h-0">
    <AppSidebar />
    <SidebarInset class="min-w-0 overflow-hidden">
      <StatusBar @command="commandOpen = true" />
      <main class="flex-1 min-h-0 overflow-y-auto">
        <RouterView />
      </main>
    </SidebarInset>
    <CommandPalette v-model:open="commandOpen" />
    <Toaster :theme="colorMode === 'light' ? 'light' : 'dark'" position="bottom-right" rich-colors close-button />
  </SidebarProvider>
</template>
