<script setup lang="ts">
import { useEventListener } from '@vueuse/core'
import { useRouter } from 'vue-router'
import { toast } from 'vue-sonner'
import { Circle, MoonStar, Radar, Rows3, Square, Sun } from '@lucide/vue'
import { errorMessage } from '@/composables/useApi'
import { usePreferences, type Density } from '@/composables/usePreferences'
import { useNodesStore } from '@/stores/nodes'
import { useRecordingsStore } from '@/stores/recordings'
import { useSourcesStore, type Source } from '@/stores/sources'
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
} from '@/components/ui/command'
import { navItems } from './navItems'

const open = defineModel<boolean>('open', { required: true })

const router = useRouter()
const sources = useSourcesStore()
const recordings = useRecordingsStore()
const nodes = useNodesStore()
const { colorMode, density } = usePreferences()

useEventListener('keydown', (e: KeyboardEvent) => {
  if (e.key.toLowerCase() === 'k' && (e.metaKey || e.ctrlKey)) {
    e.preventDefault()
    open.value = !open.value
  }
})

function run(fn: () => unknown) {
  open.value = false
  fn()
}

function sourceLabel(s: Source) {
  return nodes.nodes.length > 1 ? `${s.display_name} — ${nodes.nameOf(s.node_id)}` : s.display_name
}

async function toggleRecording(s: Source) {
  const session = recordings.activeForSource(s.node_id, s.id)
  try {
    if (session) {
      await recordings.stop(s.node_id, session.id)
      toast.success(`Stopped ${s.display_name}`)
    } else {
      await recordings.start(s.node_id, s.id, null)
      toast.success(`Recording ${s.display_name}`, { description: 'Default H.264 preset' })
    }
  } catch (e) {
    toast.error(session ? 'Stop failed' : 'Record failed', { description: errorMessage(e, '') })
  }
}

async function scan() {
  const id = toast.loading('Scanning for sources…')
  try {
    await sources.scanAll()
    toast.success(`Scan complete — ${sources.sources.length} sources`, { id })
  } catch (e) {
    toast.error('Scan failed', { id, description: errorMessage(e, '') })
  }
}

const DENSITIES: Density[] = ['compact', 'default', 'comfortable']
</script>

<template>
  <CommandDialog v-model:open="open">
    <CommandInput placeholder="Search views, sources, commands…" />
    <CommandList>
      <CommandEmpty>No results.</CommandEmpty>

      <CommandGroup heading="Go to">
        <CommandItem
          v-for="item in navItems"
          :key="item.to"
          :value="`go ${item.label}`"
          @select="run(() => router.push(item.to))"
        >
          <component :is="item.icon" />
          {{ item.label }}
        </CommandItem>
      </CommandGroup>

      <CommandSeparator />

      <CommandGroup v-if="sources.sources.length" heading="Recording">
        <CommandItem
          v-for="s in sources.sources"
          :key="s.key"
          :value="`record ${s.key} ${sourceLabel(s)}`"
          @select="run(() => toggleRecording(s))"
        >
          <template v-if="recordings.activeForSource(s.node_id, s.id)">
            <Square class="text-tally!" />
            Stop {{ sourceLabel(s) }}
          </template>
          <template v-else>
            <Circle class="text-tally!" />
            <span class="flex-1">Record {{ sourceLabel(s) }}</span>
            <span class="text-muted-foreground">default preset</span>
          </template>
        </CommandItem>
      </CommandGroup>

      <CommandSeparator />

      <CommandGroup heading="Commands">
        <CommandItem value="scan sources" @select="run(scan)">
          <Radar /> Scan for sources
        </CommandItem>
        <CommandItem
          value="toggle theme dark light"
          @select="run(() => (colorMode = colorMode === 'dark' ? 'light' : 'dark'))"
        >
          <Sun v-if="colorMode === 'dark'" /><MoonStar v-else />
          Switch to {{ colorMode === 'dark' ? 'light' : 'dark' }} theme
        </CommandItem>
        <CommandItem
          v-for="d in DENSITIES"
          :key="d"
          :value="`density ${d}`"
          @select="run(() => (density = d))"
        >
          <Rows3 /> <span class="flex-1">Density: {{ d }}</span>
          <span v-if="density === d" class="text-muted-foreground">current</span>
        </CommandItem>
      </CommandGroup>
    </CommandList>
  </CommandDialog>
</template>
