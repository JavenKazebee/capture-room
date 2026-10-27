<script setup lang="ts">
import { computed } from 'vue'
import { usePreferences } from '@/composables/usePreferences'
import { formatFingerprint, formatUptime, plural } from '@/lib/format'
import { useNodesStore } from '@/stores/nodes'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import PageHeader from '@/components/common/PageHeader.vue'
import KeyValueList from '@/components/common/KeyValueList.vue'
import AccessTokenSettings from '@/components/auth/AccessTokenSettings.vue'
import ClockSettings from '@/components/settings/ClockSettings.vue'
import MonitorSettings from '@/components/settings/MonitorSettings.vue'

const { colorMode, density } = usePreferences()
const nodes = useNodesStore()

const selfNode = computed(() => nodes.nodes.find((n) => n.is_self))
const peerCount = computed(() => nodes.nodes.length - 1)

const about = computed(() => [
  { label: 'Version', value: selfNode.value ? `v${selfNode.value.version}` : null, mono: true },
  { label: 'Node name', value: nodes.self?.node_name },
  { label: 'Node ID', value: nodes.self?.node_id, mono: true, copy: true },
  {
    label: 'TLS key',
    value: selfNode.value?.fingerprint ? formatFingerprint(selfNode.value.fingerprint) : null,
    mono: true,
    copy: true,
  },
  { label: 'API', value: `${window.location.origin}/api/v1`, mono: true, copy: true },
  {
    label: 'Controller',
    value: !nodes.self
      ? null
      : nodes.isController
        ? `On · ${plural(peerCount.value, 'other node')}`
        : 'Off',
  },
  { label: 'Uptime', value: selfNode.value ? formatUptime(selfNode.value.uptime_secs) : null, mono: true },
])
</script>

<template>
  <PageHeader title="Settings" />
  <div class="p-4 max-w-3xl space-y-6">
    <section id="appearance" class="space-y-2 scroll-mt-4">
      <div>
        <h2 class="text-base font-semibold tracking-tight">Appearance</h2>
        <p class="text-xs text-muted-foreground">Stored in this browser only.</p>
      </div>
      <div class="rounded-lg border border-border bg-card overflow-hidden">
        <div class="divide-y divide-border">
          <div class="flex items-center justify-between gap-4 px-4 py-3">
            <div>
              <div class="text-sm">Theme</div>
              <div class="text-xs text-muted-foreground">Dark is the primary theme.</div>
            </div>
            <ToggleGroup :model-value="colorMode" @update:model-value="(v) => v && (colorMode = v as typeof colorMode)" type="single" variant="segmented">
              <ToggleGroupItem value="dark" class="px-2.5">Dark</ToggleGroupItem>
              <ToggleGroupItem value="light" class="px-2.5">Light</ToggleGroupItem>
              <ToggleGroupItem value="auto" class="px-2.5">System</ToggleGroupItem>
            </ToggleGroup>
          </div>
          <div class="flex items-center justify-between gap-4 px-4 py-3">
            <div>
              <div class="text-sm">Density</div>
              <div class="text-xs text-muted-foreground">Scales text and spacing across the whole UI.</div>
            </div>
            <ToggleGroup :model-value="density" @update:model-value="(v) => v && (density = v as typeof density)" type="single" variant="segmented">
              <ToggleGroupItem value="compact" class="px-2.5">Compact</ToggleGroupItem>
              <ToggleGroupItem value="default" class="px-2.5">Default</ToggleGroupItem>
              <ToggleGroupItem value="comfortable" class="px-2.5">Comfortable</ToggleGroupItem>
            </ToggleGroup>
          </div>
        </div>
      </div>
    </section>

    <MonitorSettings />

    <ClockSettings />

    <AccessTokenSettings />

    <section id="about" class="space-y-2 scroll-mt-4">
      <div>
        <h2 class="text-base font-semibold tracking-tight">About</h2>
        <p class="text-xs text-muted-foreground">The Capture Room instance this browser is connected to.</p>
      </div>
      <div class="rounded-lg border border-border bg-card overflow-hidden">
        <div class="px-4 py-3">
          <KeyValueList :items="about" />
        </div>
        <p class="border-t border-border px-4 py-3 text-xs text-muted-foreground">
          © 2026 Javen Kazebee. Capture Room is free software under the GNU General Public License v3.0 or later, and comes
          with no warranty. Source code, third-party licenses and where to get their source:
          <a
            href="https://github.com/JavenKazebee/capture-room"
            target="_blank"
            rel="noopener noreferrer"
            class="text-foreground underline underline-offset-2 hover:text-primary"
            >github.com/JavenKazebee/capture-room</a
          >.
        </p>
      </div>
    </section>

  </div>
</template>
