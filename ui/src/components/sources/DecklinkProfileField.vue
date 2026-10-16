<script setup lang="ts">
import { ref } from 'vue'
import { toast } from 'vue-sonner'
import { useSourcesStore } from '@/stores/sources'
import { errorMessage } from '@/composables/useApi'
import { PROFILE_OPTIONS } from '@/lib/decklink'
import type { DecklinkCardDto } from '@/types/generated/DecklinkCardDto'
import type { DecklinkProfile } from '@/types/generated/DecklinkProfile'
import FormField from '@/components/FormField.vue'
import OptionSelect from '@/components/OptionSelect.vue'

/**
 * A Decklink card's profile: how its connectors are split into sub-devices.
 * A node setting, saved as soon as it's changed, not with the form.
 */
const props = defineProps<{ nodeId: string; card: DecklinkCardDto }>()
const emit = defineEmits<{ changed: [] }>()

const store = useSourcesStore()
const saving = ref(false)

async function set(v: unknown) {
  const profile = v as DecklinkProfile
  if (profile === props.card.profile || saving.value) return
  saving.value = true
  try {
    await store.setDecklinkProfile(props.nodeId, props.card.key, profile)
    toast.success(`${props.card.name} profile changed`)
    emit('changed')
  } catch (e) {
    toast.error(errorMessage(e, "Couldn't change the card's profile."))
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <FormField :label="`${card.name} profile`">
    <OptionSelect :model-value="card.profile" :options="PROFILE_OPTIONS" :disabled="saving" @update:model-value="set" />
  </FormField>
  <p class="-mt-1.5 text-xs text-muted-foreground">
    Applies to the whole card and is saved right away. Sources and outputs on the card restart, and their recordings stop.
  </p>
</template>
