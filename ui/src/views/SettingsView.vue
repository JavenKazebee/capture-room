<script setup lang="ts">
import { usePreferences } from '@/composables/usePreferences'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import PageHeader from '@/components/common/PageHeader.vue'

const { colorMode, density } = usePreferences()
</script>

<template>
  <PageHeader title="Settings" />
  <div class="p-4 max-w-3xl space-y-4">
    <section class="rounded-lg border border-border bg-card">
      <h2 class="px-4 py-2.5 border-b border-border text-sm font-semibold">Appearance</h2>
      <div class="divide-y divide-border">
        <div class="flex items-center justify-between gap-4 px-4 py-3">
          <div>
            <div class="text-sm">Theme</div>
            <div class="text-xs text-muted-foreground">Dark is the primary theme.</div>
          </div>
          <ToggleGroup :model-value="colorMode" @update:model-value="(v) => v && (colorMode = v as typeof colorMode)" type="single" variant="outline" size="sm">
            <ToggleGroupItem value="dark">Dark</ToggleGroupItem>
            <ToggleGroupItem value="light">Light</ToggleGroupItem>
            <ToggleGroupItem value="auto">System</ToggleGroupItem>
          </ToggleGroup>
        </div>
        <div class="flex items-center justify-between gap-4 px-4 py-3">
          <div>
            <div class="text-sm">Density</div>
            <div class="text-xs text-muted-foreground">Scales text and spacing across the whole UI.</div>
          </div>
          <ToggleGroup :model-value="density" @update:model-value="(v) => v && (density = v as typeof density)" type="single" variant="outline" size="sm">
            <ToggleGroupItem value="compact">Compact</ToggleGroupItem>
            <ToggleGroupItem value="default">Default</ToggleGroupItem>
            <ToggleGroupItem value="comfortable">Comfortable</ToggleGroupItem>
          </ToggleGroup>
        </div>
      </div>
    </section>
    <p class="text-xs text-muted-foreground">
      These preferences are stored in this browser only.
    </p>
  </div>
</template>
