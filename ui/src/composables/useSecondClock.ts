import { createSharedComposable, useNow } from '@vueuse/core'

/** The time now, ticking each second: one timer for every component that shows it. */
export const useSecondClock = createSharedComposable(() => useNow({ interval: 1000 }))
