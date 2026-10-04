<script setup lang="ts">
import type { VariantProps } from 'class-variance-authority'
import type { ToggleGroupRootEmits, ToggleGroupRootProps } from 'reka-ui'
import type { HTMLAttributes } from 'vue'
import type { toggleVariants } from '@/components/ui/toggle'
import { reactiveOmit } from '@vueuse/core'
import { ToggleGroupRoot, useForwardPropsEmits } from 'reka-ui'
import { provide } from 'vue'
import { cn } from '@/lib/utils'

type ToggleGroupVariants = VariantProps<typeof toggleVariants>

const props = withDefaults(defineProps<ToggleGroupRootProps & {
  class?: HTMLAttributes['class']
  variant?: ToggleGroupVariants['variant']
  size?: ToggleGroupVariants['size']
  spacing?: number
}>(), {
  spacing: undefined,
})

// Segmented pills sit apart inside their track; other variants butt together.
const spacing = props.spacing ?? (props.variant === 'segmented' ? 0.5 : 0)

const emits = defineEmits<ToggleGroupRootEmits>()

provide('toggleGroup', {
  variant: props.variant,
  size: props.size,
  spacing,
})

const delegatedProps = reactiveOmit(props, 'class', 'size', 'variant')
const forwarded = useForwardPropsEmits(delegatedProps, emits)
</script>

<template>
  <ToggleGroupRoot
    v-slot="slotProps"
    data-slot="toggle-group"
    :data-size="size"
    :data-variant="variant"
    :data-spacing="spacing"
    :style="{
      '--gap': spacing,
    }"
    v-bind="forwarded"
    :class="cn('rounded-md data-[size=sm]:rounded-[min(var(--radius-md),8px)] group/toggle-group flex w-fit flex-row items-center gap-[--spacing(var(--gap))] data-vertical:flex-col data-vertical:items-stretch data-[variant=segmented]:h-7 data-[variant=segmented]:data-[size=sm]:h-6 data-[variant=segmented]:p-0.5 data-[variant=segmented]:border data-[variant=segmented]:border-input data-[variant=segmented]:bg-input/20 dark:data-[variant=segmented]:bg-input/30', props.class)"
  >
    <slot v-bind="slotProps" />
  </ToggleGroupRoot>
</template>
