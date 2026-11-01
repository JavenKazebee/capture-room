<script setup lang="ts">
import { ref, watch } from 'vue'
import { useStorage } from '@vueuse/core'
import { nodeApi } from '@/composables/useApi'
import { useNodesStore } from '@/stores/nodes'
import type { UpdateStatusDto } from '@/types/generated/UpdateStatusDto'
import { Dialog, DialogContent, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import ReleaseNotes from '@/components/nodes/ReleaseNotes.vue'

/**
 * The highlights of the version this instance now runs, once, after it updated. The version
 * last seen is kept per browser; a browser that has seen none (a fresh install, or a new
 * browser) records the current one and shows nothing.
 */
const nodes = useNodesStore()
const seen = useStorage('cr.whatsNew.seen', '')

const open = ref(false)
const shown = ref<UpdateStatusDto | null>(null)

// The node checks for updates as it starts, so right after an update the check may not be
// done yet.
const RETRY_MS = 5_000
const RETRIES = 6

async function look(version: string, tries = RETRIES) {
  const selfId = nodes.nodes.find((n) => n.is_self)?.id
  if (!selfId || version !== nodes.version) return
  let status: UpdateStatusDto | null = null
  try {
    status = await nodeApi(selfId)<UpdateStatusDto>('/update')
  } catch {
    // Retried below.
  }
  if (version !== nodes.version) return
  if (!status?.latest || status.phase === 'checking') {
    if (tries > 0) setTimeout(() => look(version, tries - 1), RETRY_MS)
    return
  }
  seen.value = version
  if (status.latest === version && status.notes) {
    shown.value = status
    open.value = true
  }
}

watch(
  () => nodes.version,
  (version) => {
    if (!version || version === seen.value) return
    if (!seen.value) seen.value = version
    else look(version)
  },
  { immediate: true },
)
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent v-if="shown?.notes" class="sm:max-w-lg">
      <DialogHeader>
        <DialogTitle>What's new in <span class="num">{{ shown.latest }}</span></DialogTitle>
      </DialogHeader>
      <ReleaseNotes :notes="shown.notes" :url="shown.notes_url" />
    </DialogContent>
  </Dialog>
</template>
