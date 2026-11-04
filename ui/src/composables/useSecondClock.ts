import { createSharedComposable, useIntervalFn, useNow } from '@vueuse/core'

/** The time now, ticking each second: one timer for every component that shows it. */
export const useSecondClock = createSharedComposable(() =>
  useNow({ scheduler: (cb) => useIntervalFn(cb, 1000) }),
)
