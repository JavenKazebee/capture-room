import { watch } from 'vue'
import { useRoute, useRouter, type LocationQueryValue } from 'vue-router'

type QueryValue = LocationQueryValue | LocationQueryValue[] | undefined

/**
 * Act once on a deep link such as `?select=<id>` from the command palette, then clear the
 * query so a reload or Back doesn't act again. Runs while `when` is truthy, and again each
 * time its value changes; `handler` returns `false` to leave the query for a later run
 * (the row it wants hasn't loaded yet).
 */
export function useQueryAction<K extends string>(
  keys: K[],
  handler: (query: Record<K, QueryValue>) => unknown,
  { when = () => true }: { when?: () => unknown } = {},
) {
  const route = useRoute()
  const router = useRouter()
  watch(
    () => [...keys.map((k) => route.query[k]), when()] as const,
    async () => {
      if (!when() || keys.every((k) => route.query[k] === undefined)) return
      const query = Object.fromEntries(keys.map((k) => [k, route.query[k]])) as Record<K, QueryValue>
      if ((await handler(query)) === false) return
      router.replace({ query: {} })
    },
    { immediate: true },
  )
}
