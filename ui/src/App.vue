<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { RouterView } from 'vue-router'
import { useIntervalFn } from '@vueuse/core'
import { startWebSocket } from '@/composables/useWebSocket'
import { usePreferences } from '@/composables/usePreferences'
import { usePresetsStore } from '@/stores/presets'
import { useRecordingsStore } from '@/stores/recordings'
import { useSourcesStore } from '@/stores/sources'
import { useStorageStore } from '@/stores/storage'
import { Toaster } from '@/components/ui/sonner'
import { TooltipProvider } from '@/components/ui/tooltip'
import AppHeader from '@/components/app/AppHeader.vue'
import SignIn from '@/components/auth/SignIn.vue'
import { authStatus, checkAuth, needsSignIn } from '@/composables/useAuth'
import BottomPanel from '@/components/app/BottomPanel.vue'
import CommandPalette from '@/components/app/CommandPalette.vue'

const { colorMode } = usePreferences()
const storage = useStorageStore()
const commandOpen = ref(false)

onMounted(async () => {
  await checkAuth()
  if (needsSignIn.value) return
  startWebSocket()
  usePresetsStore().load()
  // Shared with a page that asked first (its onMounted runs before this).
  await useSourcesStore().ensureLoaded()
  useRecordingsStore().load()
  storage.load()
})

// Free space changes slowly and isn't evented; poll it for the header and Storage tab.
useIntervalFn(() => storage.load(), 30_000)
</script>

<template>
  <TooltipProvider :delay-duration="300">
    <SignIn v-if="needsSignIn" />
    <template v-else-if="authStatus">
      <div class="h-svh flex flex-col bg-background text-foreground">
        <AppHeader @command="commandOpen = true" />
        <main class="flex-1 min-h-0 overflow-hidden">
          <RouterView />
        </main>
        <BottomPanel />
      </div>

      <CommandPalette v-model:open="commandOpen" />
    </template>
    <Toaster :theme="colorMode === 'light' ? 'light' : 'dark'" position="bottom-right" :offset="44" rich-colors close-button />
  </TooltipProvider>
</template>
