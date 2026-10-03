import { FetchError, ofetch } from 'ofetch'

export const api = ofetch.create({ baseURL: '/api/v1' })

/**
 * Talk to one node's local API. Works for this instance and, when it is acting
 * as a controller, for any peer — the server forwards `/nodes/{id}/…` for us.
 */
export function nodeApi(nodeId: string) {
  return ofetch.create({ baseURL: `/api/v1/nodes/${encodeURIComponent(nodeId)}` })
}

/** Stable key for a source across nodes (source ids are only unique per node). */
export function sourceKey(nodeId: string, sourceId: string) {
  return `${nodeId}/${sourceId}`
}

export function thumbnailUrl(nodeId: string, sourceId: string) {
  return `/api/v1/nodes/${encodeURIComponent(nodeId)}/thumbnails/${encodeURIComponent(sourceId)}`
}

/**
 * A failed request's message for display. The server replies with plain-text
 * error bodies; ofetch's own `message` is only "[POST] /url: 500 …".
 */
export function errorMessage(e: unknown, fallback: string): string {
  if (e instanceof FetchError && typeof e.data === 'string' && e.data.trim()) return e.data
  return e instanceof Error ? e.message : fallback
}
