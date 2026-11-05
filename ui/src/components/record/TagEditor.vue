<script setup lang="ts">
import { computed, ref } from 'vue'
import { Plus, X } from '@lucide/vue'
import { useLayoutStore } from '@/stores/layout'
import { Input } from '@/components/ui/input'

/** Tags on one or several feeds. A tag only some of them have is dashed: click it to give it to all. */
const props = defineProps<{ keys: string[] }>()

const layout = useLayoutStore()
const draft = ref('')

/** Every tag on any of the feeds, with how many have it. */
const present = computed(() => {
  const counts = new Map<string, { tag: string; n: number }>()
  for (const k of props.keys) {
    for (const t of layout.tagsOf(k)) {
      const c = counts.get(t.toLowerCase())
      if (c) c.n++
      else counts.set(t.toLowerCase(), { tag: t, n: 1 })
    }
  }
  return [...counts.values()].sort((a, b) => a.tag.localeCompare(b.tag))
})

const suggestions = computed(() => {
  const q = draft.value.trim().toLowerCase()
  const onAll = new Set(present.value.filter((p) => p.n === props.keys.length).map((p) => p.tag.toLowerCase()))
  return layout.allTags.filter((t) => !onAll.has(t.toLowerCase()) && (!q || t.toLowerCase().includes(q))).slice(0, 8)
})

function add(tag: string) {
  const t = tag.trim()
  if (!t) return
  layout.changeTags(props.keys, [t])
  draft.value = ''
}

function remove(tag: string) {
  layout.changeTags(props.keys, [], [tag])
}
</script>

<template>
  <div class="flex flex-col gap-2.5">
    <p class="text-xs font-medium">
      Tags<span v-if="keys.length > 1" class="text-muted-foreground font-normal"> · {{ keys.length }} selected feeds</span>
    </p>
    <div v-if="present.length" class="flex flex-wrap gap-1">
      <span
        v-for="p in present"
        :key="p.tag"
        class="inline-flex items-center gap-0.5 rounded-full border pl-2 pr-0.5 h-6 text-xs"
        :class="p.n === keys.length ? 'bg-secondary border-transparent' : 'border-dashed text-muted-foreground'"
      >
        <button
          v-if="p.n < keys.length"
          class="hover:text-foreground"
          :title="`On ${p.n} of ${keys.length}: add to all`"
          @click="add(p.tag)"
        >
          {{ p.tag }}
        </button>
        <template v-else>{{ p.tag }}</template>
        <button
          class="size-5 grid place-items-center rounded-full text-muted-foreground hover:text-foreground hover:bg-accent"
          :aria-label="`Remove ${p.tag}`"
          @click="remove(p.tag)"
        >
          <X class="size-3" />
        </button>
      </span>
    </div>
    <p v-else class="text-xs text-muted-foreground">No tags yet. Feeds that share a tag can be filtered, selected and recorded together.</p>
    <form class="flex gap-1.5" @submit.prevent="add(draft)">
      <Input v-model="draft" placeholder="Add a tag" maxlength="40" class="h-7 text-xs" aria-label="New tag" />
    </form>
    <div v-if="suggestions.length" class="flex flex-wrap gap-1">
      <button
        v-for="t in suggestions"
        :key="t"
        class="inline-flex items-center gap-1 rounded-full border border-dashed h-6 px-2 text-xs text-muted-foreground hover:text-foreground hover:border-foreground/40"
        @click="add(t)"
      >
        <Plus class="size-3" /> {{ t }}
      </button>
    </div>
  </div>
</template>
