import { shallowReactive } from 'vue'
import { needsSignIn } from '@/composables/useAuth'
import { sourceKey } from '@/composables/useApi'
import { reconnectingSocket } from '@/lib/reconnectingSocket'
import type { ThumbnailSubscription } from '@/types/generated/ThumbnailSubscription'

/**
 * One socket for every thumbnail on the page (`/ws/thumbnails`). It's told
 * which sources are watched (shown on screen, while the page is visible), and
 * pushes each new JPEG as `key length (u16) | "node/source" | JPEG`.
 */

/** Object URLs of the latest frame, by sourceKey. */
export const thumbnailFrames = shallowReactive(new Map<string, string>())

/** Watchers per sourceKey, and its node and source ids. */
const watched = new Map<string, { count: number; node_id: string; source_id: string }>()
/** Each source's frame before the latest: freed once the next one arrives, so an <img> still loading it doesn't break. */
const previous = new Map<string, string>()
let sendQueued = false

/** Show `source_id`'s thumbnail until the returned function is called. */
export function watchThumbnail(nodeId: string, sourceId: string): () => void {
  const key = sourceKey(nodeId, sourceId)
  const entry = watched.get(key)
  if (entry) entry.count++
  else watched.set(key, { count: 1, node_id: nodeId, source_id: sourceId })
  queueSend()
  return () => {
    const entry = watched.get(key)
    if (!entry || --entry.count > 0) return
    watched.delete(key)
    drop(key)
    queueSend()
  }
}

function drop(key: string) {
  for (const urls of [thumbnailFrames, previous]) {
    const url = urls.get(key)
    if (url) URL.revokeObjectURL(url)
    urls.delete(key)
  }
}

/** Send the subscription once per tick, however many watchers changed. */
function queueSend() {
  if (sendQueued) return
  sendQueued = true
  queueMicrotask(() => {
    sendQueued = false
    send()
  })
}

const socket = reconnectingSocket({
  path: '/ws/thumbnails',
  shouldReconnect: () => watched.size > 0 && !needsSignIn.value,
  onOpen: send,
  onBinary: receive,
})

function send() {
  const sub: ThumbnailSubscription = {
    sources: document.hidden ? [] : [...watched.values()].map(({ node_id, source_id }) => ({ node_id, source_id })),
    fps: null,
  }
  if (!socket.send(JSON.stringify(sub))) socket.connect()
}

const decoder = new TextDecoder()

function receive(data: ArrayBuffer) {
  if (data.byteLength < 2) return
  const keyLength = new DataView(data).getUint16(0)
  const key = decoder.decode(new Uint8Array(data, 2, keyLength))
  // Arrived after it stopped being watched.
  if (!watched.has(key)) return
  const url = URL.createObjectURL(new Blob([new Uint8Array(data, 2 + keyLength)], { type: 'image/jpeg' }))
  const old = previous.get(key)
  if (old) URL.revokeObjectURL(old)
  const current = thumbnailFrames.get(key)
  if (current) previous.set(key, current)
  thumbnailFrames.set(key, url)
}

// A hidden page watches nothing; showing it again resubscribes.
document.addEventListener('visibilitychange', queueSend)
