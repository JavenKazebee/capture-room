<script setup lang="ts">
import { ExternalLink } from '@lucide/vue'
import { renderNotes } from '@/lib/notes'

/** A release's highlights, and a link to the whole release page. */
const props = defineProps<{ notes: string; url?: string | null }>()

const Body = () => renderNotes(props.notes)
</script>

<template>
  <div class="flex flex-col gap-2 text-sm leading-relaxed">
    <Body />
    <a
      v-if="url"
      :href="url"
      target="_blank"
      rel="noopener noreferrer"
      class="inline-flex items-center gap-1 self-start text-xs text-muted-foreground hover:text-foreground"
    >
      Full release notes <ExternalLink class="size-3" />
    </a>
  </div>
</template>
