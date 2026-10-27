<script setup lang="ts">
import { computed, ref } from 'vue'
import { toast } from 'vue-sonner'
import { notifyError } from '@/lib/notify'
import { useNodesStore } from '@/stores/nodes'
import { useSettingsStore } from '@/stores/settings'
import type { UpdateChannel } from '@/types/generated/UpdateChannel'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'

/** Which releases this node offers as updates. Nodes are updated to their controller's version, so it's set here only. */
const nodes = useNodesStore()
const settings = useSettingsStore()

const channel = computed(() => nodes.self?.update_channel ?? null)
const saving = ref(false)

async function onChannel(value: unknown) {
  if ((value !== 'stable' && value !== 'prerelease') || value === channel.value) return
  saving.value = true
  try {
    await settings.setUpdateChannel(value as UpdateChannel)
    toast.success(value === 'prerelease' ? 'Pre-releases will be offered as updates' : 'Only stable releases will be offered')
  } catch (e) {
    notifyError("Couldn't change the update channel", e)
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <section v-if="channel" id="updates" class="space-y-2 scroll-mt-4">
    <div>
      <h2 class="text-base font-semibold tracking-tight">Updates</h2>
      <p class="text-xs text-muted-foreground">
        Updates are offered on the Nodes page, never installed on their own.
        <template v-if="nodes.isController">Nodes are updated to this controller's version.</template>
      </p>
    </div>
    <div class="rounded-lg border border-border bg-card overflow-hidden">
      <div class="flex items-center justify-between gap-4 px-4 py-3">
        <div>
          <div class="text-sm">Release channel</div>
          <div class="text-xs text-muted-foreground">
            Pre-releases are betas and release candidates, for testing. Switching back to stable keeps the version
            installed until a newer stable release comes out.
          </div>
        </div>
        <ToggleGroup
          :model-value="channel"
          type="single"
          variant="segmented"
          :disabled="saving"
          @update:model-value="onChannel"
        >
          <ToggleGroupItem value="stable" class="px-2.5">Stable</ToggleGroupItem>
          <ToggleGroupItem value="prerelease" class="px-2.5">Pre-releases</ToggleGroupItem>
        </ToggleGroup>
      </div>
    </div>
  </section>
</template>
