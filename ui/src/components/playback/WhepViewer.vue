<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { watchWhep, type WhepSession } from '@/lib/whep'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import CopyButton from '@/components/common/CopyButton.vue'

/** Watch a channel's WHEP output in the browser, as any viewer would. */
const props = defineProps<{ title: string; endpoint: string }>()
const emit = defineEmits<{ close: [] }>()

const video = ref<HTMLVideoElement | null>(null)
const error = ref<string | null>(null)
const connection = ref<RTCPeerConnectionState>('new')
let session: WhepSession | null = null
let closed = false

onMounted(async () => {
  try {
    const s = await watchWhep(props.endpoint)
    if (closed) return s.close()
    session = s
    s.pc.onconnectionstatechange = () => (connection.value = s.pc.connectionState)
    if (video.value) video.value.srcObject = s.stream
  } catch (e) {
    error.value = e instanceof Error ? e.message : String(e)
  }
})

onBeforeUnmount(() => {
  closed = true
  session?.close()
})
</script>

<template>
  <Dialog :open="true" @update:open="(v) => !v && emit('close')">
    <DialogContent class="sm:max-w-3xl flex flex-col gap-3">
      <DialogHeader>
        <DialogTitle>{{ title }}</DialogTitle>
        <DialogDescription class="flex flex-wrap items-center gap-1">
          Over WHEP from <span class="font-mono break-all">{{ endpoint }}</span>
          <CopyButton :value="endpoint" />
        </DialogDescription>
      </DialogHeader>
      <div class="relative bg-black aspect-video rounded-md overflow-hidden">
        <!-- Muted until clicked: browsers only autoplay without sound. -->
        <video ref="video" class="w-full h-full object-contain" autoplay playsinline muted controls />
        <div
          v-if="error || connection !== 'connected'"
          class="absolute inset-0 grid place-items-center px-6 text-center text-xs pointer-events-none"
          :class="error || connection === 'failed' ? 'text-destructive' : 'text-zinc-400'"
        >
          <template v-if="error">{{ error }}</template>
          <template v-else-if="connection === 'failed'">
            The connection failed: the browser couldn't reach the node over UDP.
          </template>
          <template v-else>Connecting…</template>
        </div>
      </div>
    </DialogContent>
  </Dialog>
</template>
