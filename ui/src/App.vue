<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { RouterView } from 'vue-router'
import { useIntervalFn } from '@vueuse/core'
import { reloadAll, startWebSocket } from '@/composables/useWebSocket'
import { usePreferences } from '@/composables/usePreferences'
import { usePresetsStore } from '@/stores/presets'
import { useStorageStore } from '@/stores/storage'
import { Toaster } from '@/components/ui/sonner'
import { TooltipProvider } from '@/components/ui/tooltip'
import AppHeader from '@/components/app/AppHeader.vue'
import BottomPanel from '@/components/app/BottomPanel.vue'
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

// Free space changes slowly and isn't evented; poll it for the header and Storage tab.
useIntervalFn(() => storage.load(), 30_000)
</script>

<template>
  <TooltipProvider :delay-duration="300">
    <div class="h-svh flex flex-col bg-background text-foreground">
      <AppHeader @command="commandOpen = true" />
      <main class="flex-1 min-h-0 overflow-hidden">
        <RouterView />
      </main>
      <BottomPanel />
    </div>

    <CommandPalette v-model:open="commandOpen" />
    <Toaster :theme="colorMode === 'light' ? 'light' : 'dark'" position="bottom-right" :offset="44" rich-colors close-button />
  </TooltipProvider>
</template>
