<script setup lang="ts">
import { computed, ref } from 'vue'
import { ChevronRight, TriangleAlert } from '@lucide/vue'
import type { WhipSourceConfig } from '@/types/generated/WhipSourceConfig'
import type { DeviceDto } from '@/types/generated/DeviceDto'
import { Input } from '@/components/ui/input'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import FormField from '@/components/FormField.vue'
import CopyButton from '@/components/common/CopyButton.vue'
import AudioPlanField from './AudioPlanField.vue'
import LiveFormatField from './LiveFormatField.vue'

/** A WHIP endpoint on the node that OBS or a browser publishes to. */
const props = defineProps<{
  /** Where publishers reach the node. */
  host: string
  devices: DeviceDto[]
}>()

const cfg = defineModel<WhipSourceConfig>({ required: true })

const endpoint = computed(() => `http://${props.host}:${cfg.value.port || '…'}/whip/endpoint`)
const advancedOpen = ref(false)
</script>

<template>
  <div class="col-span-2 flex flex-col gap-3">
    <FormField label="Port">
      <Input v-model.number="cfg.port" type="number" min="1024" max="65535" class="w-32" />
    </FormField>

    <div class="rounded-md border border-border bg-muted/40 px-2.5 py-2 text-xs flex flex-col gap-1">
      <span class="text-muted-foreground">Publish to</span>
      <span class="flex items-center gap-1 font-mono break-all">
        {{ endpoint }}
        <CopyButton :value="endpoint" />
      </span>
      <span class="text-muted-foreground">
        In OBS 30+: Settings › Stream, Service WHIP, Server set to this address. One publisher at a time.
      </span>
    </div>
    <p class="flex items-start gap-1.5 text-xs text-muted-foreground">
      <TriangleAlert class="size-3.5 shrink-0 mt-px text-warning" />
      No authentication yet: anyone who can reach this port can publish.
    </p>

    <AudioPlanField v-model="cfg.audio" :devices="devices" source-label="From publisher" />

    <Collapsible v-model:open="advancedOpen">
      <CollapsibleTrigger class="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground">
        <ChevronRight class="size-3.5 transition-transform" :class="advancedOpen && 'rotate-90'" />
        Advanced
        <span v-if="!advancedOpen" class="opacity-70">— {{ cfg.format ? 'fixed format' : "publisher's format" }}</span>
      </CollapsibleTrigger>
      <CollapsibleContent class="flex flex-col gap-3 pt-3">
        <LiveFormatField v-model="cfg.format" />
      </CollapsibleContent>
    </Collapsible>
  </div>
</template>
