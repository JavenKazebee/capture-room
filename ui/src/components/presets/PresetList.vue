<script setup lang="ts">
import { computed, ref } from 'vue'
import { Lock, Search } from '@lucide/vue'
import { plural } from '@/lib/format'
import { CODECS } from '@/lib/codecs'
import { DEFAULT_PRESET, usePresetsStore } from '@/stores/presets'
import type { PresetDto } from '@/types/generated/PresetDto'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'
import { Input } from '@/components/ui/input'
import LoadError from '@/components/common/LoadError.vue'

/** The presets sidebar: the built-in default, an unsaved new one, then saved ones by name. */
const props = defineProps<{
  presets: PresetDto[]
  selectedId: string | null
  /** The draft's name, shown on an unsaved new preset. */
  draftName: string
  dirty: boolean
  /** How many Record feeds use a preset. */
  usedBy: (id: string) => number
}>()
const emit = defineEmits<{ select: [id: string] }>()

const store = usePresetsStore()

const query = ref('')
const listed = computed(() => {
  const q = query.value.trim().toLowerCase()
  return props.presets.filter((p) => !q || p.name.toLowerCase().includes(q))
})

function codecsOf(outputs: PresetOutputInput[]) {
  return [...new Set(outputs.map((o) => CODECS[o.codec]))].join(' + ')
}
</script>

<template>
  <aside class="w-64 shrink-0 border-r border-border flex flex-col min-h-0">
    <div class="p-2 border-b border-border relative">
      <Search class="absolute left-4 top-1/2 -translate-y-1/2 size-3.5 text-muted-foreground" />
      <Input v-model="query" placeholder="Filter presets…" class="h-7 pl-7 text-xs" />
    </div>
    <LoadError :error="store.error" :retry="store.load" class="px-3 py-2 border-b border-border" />
    <nav class="flex-1 overflow-y-auto p-1.5 space-y-0.5">
      <button
        class="preset-item"
        :data-active="selectedId === DEFAULT_PRESET.id || undefined"
        @click="emit('select', DEFAULT_PRESET.id)"
      >
        <span class="flex items-center gap-1.5 text-sm font-medium"><Lock class="size-3 text-muted-foreground" /> {{ DEFAULT_PRESET.name }}</span>
        <span class="text-[11px] text-muted-foreground">Built in · 1 output · H.264</span>
      </button>
      <button
        v-if="selectedId === 'new'"
        class="preset-item"
        data-active
      >
        <span class="text-sm font-medium italic">{{ draftName || 'New preset' }}</span>
        <span class="text-[11px] text-muted-foreground">Unsaved</span>
      </button>
      <button
        v-for="p in listed"
        :key="p.id"
        class="preset-item"
        :data-active="selectedId === p.id || undefined"
        @click="emit('select', p.id)"
      >
        <span class="text-sm font-medium truncate w-full">
          {{ p.name }}
          <span v-if="selectedId === p.id && dirty" class="text-primary" title="Unsaved changes">•</span>
        </span>
        <span class="text-[11px] text-muted-foreground truncate w-full">
          {{ plural(p.outputs.length, 'output') }} · {{ codecsOf(p.outputs) }}
          <template v-if="usedBy(p.id)"> · on {{ plural(usedBy(p.id), 'feed') }}</template>
        </span>
      </button>
      <p v-if="!listed.length && query" class="px-2 py-3 text-xs text-muted-foreground">No presets match.</p>
    </nav>
  </aside>
</template>

<style scoped>
@reference "@/style.css";
.preset-item {
  @apply w-full flex flex-col items-start gap-0.5 rounded-md px-2.5 py-1.5 text-left hover:bg-accent/60;
}
.preset-item[data-active] {
  @apply bg-primary/10 ring-1 ring-inset ring-primary/40;
}
</style>
