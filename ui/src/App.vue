<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { RouterView } from 'vue-router'
import { useEventListener, useIntervalFn } from '@vueuse/core'
import { reloadAll, startWebSocket } from '@/composables/useWebSocket'
import { usePreferences } from '@/composables/usePreferences'
import { usePresetsStore } from '@/stores/presets'
import { useStorageStore } from '@/stores/storage'
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from '@/components/ui/resizable'
import { Toaster } from '@/components/ui/sonner'
import { TooltipProvider } from '@/components/ui/tooltip'
import AppRail from '@/components/app/AppRail.vue'
import StatusLine from '@/components/app/StatusLine.vue'
import EventLog from '@/components/app/EventLog.vue'
import CommandPalette from '@/components/app/CommandPalette.vue'

const { colorMode } = usePreferences()
const storage = useStorageStore()
const commandOpen = ref(false)

// ── Event log panel ───────────────────────────────────────────────────────────

const logPanel = ref<{ collapse(): void; expand(): void; isCollapsed: boolean }>()
const logOpen = ref(false)


onMounted(async () => {
  // The panel may come back collapsed or open from the saved layout.
  logOpen.value = !logPanel.value?.isCollapsed
  startWebSocket()
  usePresetsStore().load()
  await reloadAll()
  storage.load()
})

// Free space changes slowly and isn't evented; poll it for the status line.
useIntervalFn(() => storage.load(), 30_000)

function toggleLog() {
  if (logOpen.value) logPanel.value?.collapse()
  else logPanel.value?.expand()
}

useEventListener('keydown', (e: KeyboardEvent) => {
  if (e.key.toLowerCase() === 'j' && (e.metaKey || e.ctrlKey)) {
    e.preventDefault()
    toggleLog()
  }
})
</script>

<template>
  <TooltipProvider :delay-duration="300">
    <div class="h-svh flex flex-col bg-background text-foreground">
      <div class="flex-1 min-h-0 flex">
        <AppRail @command="commandOpen = true" />

        <ResizablePanelGroup direction="vertical" auto-save-id="cr.workbench" class="flex-1 min-w-0">
          <ResizablePanel :order="1" :min-size="30">
            <main class="h-full overflow-y-auto">
              <RouterView />
            </main>
          </ResizablePanel>
          <ResizableHandle />
          <ResizablePanel
            ref="logPanel"
            :order="2"
            :default-size="22"
            :min-size="12"
            :collapsed-size="0"
            collapsible
            @collapse="logOpen = false"
            @expand="logOpen = true"
          >
            <EventLog v-if="logOpen" @close="toggleLog" />
          </ResizablePanel>
        </ResizablePanelGroup>
      </div>

      <StatusLine :log-open="logOpen" @toggle-log="toggleLog" />
    </div>

    <CommandPalette v-model:open="commandOpen" />
    <Toaster :theme="colorMode === 'light' ? 'light' : 'dark'" position="bottom-right" :offset="36" rich-colors close-button />
  </TooltipProvider>
</template>
