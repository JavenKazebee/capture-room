<script setup lang="ts">
import { ref } from 'vue'
import { FolderOpen } from '@lucide/vue'
import type { MediaInfo } from '@/types/generated/MediaInfo'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import FormField from '@/components/FormField.vue'
import { fpsLabel } from '@/lib/sourceFormat'
import { formatDuration } from '@/lib/format'
import FileBrowseDialog from './FileBrowseDialog.vue'

/** A media file on the node's disk, played in a loop as a live feed. */
defineProps<{ nodeId: string }>()

const path = defineModel<string>('path', { required: true })
/** What the node found in the file when it was last saved; cleared when the path changes. */
const media = defineModel<MediaInfo | null>('media', { required: true })

const browsing = ref(false)

function summary(m: MediaInfo) {
  const parts = [`${m.width}×${m.height}`, fpsLabel([m.fps_num, m.fps_den])]
  parts.push(m.audio_channels ? `${m.audio_channels} ch audio` : 'no audio (plays silence)')
  if (m.duration_ms != null) parts.push(`${formatDuration(m.duration_ms)} loop`)
  return parts.join(' · ')
}
</script>

<template>
  <FormField label="File" class="col-span-2">
    <div class="flex gap-1.5">
      <Input
        v-model="path"
        class="font-mono text-[0.6875rem]"
        placeholder="/path/on/the/node/clip.mov"
        @update:model-value="media = null"
      />
      <Button type="button" variant="outline" class="gap-1.5" @click="browsing = true">
        <FolderOpen class="size-3.5" /> Browse
      </Button>
    </div>
  </FormField>
  <p class="col-span-2 text-xs text-muted-foreground">
    <template v-if="media">{{ summary(media) }}</template>
    <template v-else>The node checks the file when you save. Files without audio play silence.</template>
  </p>

  <FileBrowseDialog
    v-if="browsing"
    :node-id="nodeId"
    :start="path"
    @close="browsing = false"
    @select="(p) => { path = p; media = null }"
  />
</template>
