<script setup lang="ts">
import { Columns3 } from '@lucide/vue'
import { usePreferences } from '@/composables/usePreferences'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'

/** Show/hide a DataTable's columns; remembered per browser under `tableId`. */
const props = defineProps<{
  tableId: string
  columns: { id: string; label: string; hiddenByDefault?: boolean; alwaysVisible?: boolean }[]
}>()

const { columns: prefs } = usePreferences()

function isShown(c: (typeof props.columns)[number]) {
  return prefs.value[props.tableId]?.[c.id] ?? !c.hiddenByDefault
}

function set(id: string, shown: boolean) {
  prefs.value = { ...prefs.value, [props.tableId]: { ...prefs.value[props.tableId], [id]: shown } }
}

function reset() {
  const { [props.tableId]: _, ...rest } = prefs.value
  prefs.value = rest
}
</script>

<template>
  <DropdownMenu>
    <DropdownMenuTrigger as-child>
      <Button variant="outline" size="sm" class="h-7 gap-1.5 text-xs"><Columns3 class="size-3.5" /> Columns</Button>
    </DropdownMenuTrigger>
    <DropdownMenuContent align="end" class="w-48">
      <DropdownMenuLabel>Show columns</DropdownMenuLabel>
      <DropdownMenuCheckboxItem
        v-for="c in columns.filter((c) => !c.alwaysVisible)"
        :key="c.id"
        :model-value="isShown(c)"
        @update:model-value="(v) => set(c.id, !!v)"
        @select.prevent
      >
        {{ c.label }}
      </DropdownMenuCheckboxItem>
      <DropdownMenuSeparator />
      <DropdownMenuItem @select="reset">Reset to default</DropdownMenuItem>
    </DropdownMenuContent>
  </DropdownMenu>
</template>
