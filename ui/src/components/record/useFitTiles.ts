import { computed, ref, watch, type Ref } from 'vue'
import { useResizeObserver } from '@vueuse/core'

/**
 * The largest tile width that fits `count` tiles in the container without
 * scrolling, like a hardware multiviewer. A tile is a 16:9 picture plus a
 * caption whose height is measured from a rendered tile.
 */
export function useFitTiles(
  container: Ref<HTMLElement | undefined>,
  count: Ref<number>,
  opts: { gap: number; min: number; max: number },
) {
  // The container's content box. Read once on mount as well as on resize:
  // a ResizeObserver's first report waits for a rendered frame.
  const width = ref(0)
  const height = ref(0)
  function read() {
    const el = container.value
    if (!el) return
    const cs = getComputedStyle(el)
    width.value = el.clientWidth - parseFloat(cs.paddingLeft) - parseFloat(cs.paddingRight)
    height.value = el.clientHeight - parseFloat(cs.paddingTop) - parseFloat(cs.paddingBottom)
  }
  useResizeObserver(container, read)
  watch(container, read, { flush: 'post', immediate: true })
  /** Height of a tile below its picture: name and controls. Measured once tiles render. */
  const caption = ref(76)

  const size = computed(() => {
    const n = count.value
    const { gap, min, max } = opts
    if (!n || !width.value || !height.value) return min
    let best = 0
    for (let cols = 1; cols <= n; cols++) {
      const rows = Math.ceil(n / cols)
      const byWidth = (width.value - (cols - 1) * gap) / cols
      const byHeight = (((height.value - (rows - 1) * gap) / rows - caption.value) * 16) / 9
      best = Math.max(best, Math.min(byWidth, byHeight))
    }
    return Math.floor(Math.min(Math.max(best, min), max))
  })

  /**
   * Re-measures the caption from a rendered tile (its height beyond the 16:9
   * picture). Only a tile already laid out at the fitted width counts: one
   * caught mid-layout reads wildly tall and would pin the size at `min`.
   */
  function measure(tile: Element | null | undefined) {
    if (!(tile instanceof HTMLElement) || Math.abs(tile.offsetWidth - size.value) > 2) return
    const h = Math.round(tile.offsetHeight - (tile.offsetWidth * 9) / 16)
    if (h > 0 && h < 400 && Math.abs(h - caption.value) > 1) caption.value = h
  }

  // Tiles reflow after a size change; measure the new caption once they have.
  watch([size, count, container, width, height], () => measure(container.value?.querySelector('[data-feed-tile]')), {
    flush: 'post',
    immediate: true,
  })

  return { size, measure }
}
