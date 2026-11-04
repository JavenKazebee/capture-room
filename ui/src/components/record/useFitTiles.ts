import { computed, ref, watch, type Ref } from 'vue'
import { useResizeObserver } from '@vueuse/core'

/** The largest tile width fitting `count` tiles without scrolling; a tile is 16:9 plus a measured caption. */
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
  /** Height of a tile's caption box below its picture: name and controls. Measured once tiles render. */
  const caption = ref(74)
  /** A tile's border, top plus bottom. The picture sits inside it, so it narrows the picture too. */
  const frame = ref(2)

  const size = computed(() => {
    const n = count.value
    const { gap, min, max } = opts
    if (!n || !width.value || !height.value) return min
    let best = 0
    for (let cols = 1; cols <= n; cols++) {
      const rows = Math.ceil(n / cols)
      const byWidth = (width.value - (cols - 1) * gap) / cols
      const byHeight = (((height.value - (rows - 1) * gap) / rows - caption.value - frame.value) * 16) / 9 + frame.value
      best = Math.max(best, Math.min(byWidth, byHeight))
    }
    return Math.floor(Math.min(Math.max(best, min), max))
  })

  /** Unrounded: tiles land on fractional pixels, and anything drawn as picture plus `caption` should match exactly. */
  function measure(tile: Element | null | undefined) {
    const picture = tile?.firstElementChild
    if (!(tile instanceof HTMLElement) || !(picture instanceof HTMLElement)) return
    const border = tile.offsetHeight - tile.clientHeight
    const h = tile.getBoundingClientRect().height - picture.getBoundingClientRect().height - border
    if (h > 0 && h < 400 && Math.abs(h - caption.value) > 0.01) caption.value = h
    frame.value = border
  }

  // Tiles reflow after a size change; measure the new caption once they have.
  watch([size, count, container, width, height], () => measure(container.value?.querySelector('[data-feed-tile]')), {
    flush: 'post',
    immediate: true,
  })

  return { size, caption, measure, width }
}
