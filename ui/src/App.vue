<script setup lang="ts">
import { onMounted, ref, watch } from 'vue'
import { RouterView, useRouter } from 'vue-router'
import { useDocumentVisibility, useEventListener, useIntervalFn } from '@vueuse/core'
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
import CommandPalette from '@/components/app/CommandPalette.vue'
import DisconnectedOverlay from '@/components/app/DisconnectedOverlay.vue'

const { colorMode } = usePreferences()
const storage = useStorageStore()
const commandOpen = ref(false)
const router = useRouter()

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

// Free space changes slowly and isn't evented; poll it for the header and node
// cards while the tab is showing, and catch up when it shows again.
const storagePoll = useIntervalFn(() => storage.load(), 30_000)
watch(useDocumentVisibility(), (visibility) => {
  if (visibility === 'visible') {
    storage.load()
    storagePoll.resume()
  } else {
    storagePoll.pause()
  }
})

useEventListener('keydown', (e: KeyboardEvent) => {
  if (e.key.toLowerCase() === 'j' && (e.metaKey || e.ctrlKey)) {
    e.preventDefault()
    router.push('/setup/logs')
  }
})
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
      </div>

      <CommandPalette v-model:open="commandOpen" />
      <DisconnectedOverlay />
    </template>
    <Toaster :theme="colorMode === 'light' ? 'light' : 'dark'" position="bottom-right" rich-colors close-button />
  </TooltipProvider>
</template>
