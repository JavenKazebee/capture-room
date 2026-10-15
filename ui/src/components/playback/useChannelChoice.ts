import { computed } from 'vue'
import { useStorage } from '@vueuse/core'
import { usePlayoutStore } from '@/stores/playout'

/**
 * The channel a page works on, remembered under `storageKey`: the one
 * picked while it exists, else the first.
 */
export function useChannelChoice(storageKey: string) {
  const playout = usePlayoutStore()
  const channelKey = useStorage(storageKey, '')
  const channel = computed(
    () => playout.channels.find((c) => c.key === channelKey.value) ?? playout.channels[0] ?? null,
  )
  /** For a picker: the channel in use, also before one's been picked. */
  const choice = computed({
    get: () => channel.value?.key ?? '',
    set: (key: string) => (channelKey.value = key),
  })
  return { channelKey, channel, choice }
}
