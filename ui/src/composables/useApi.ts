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
 * A finished session's file, by output and file index (a split output has
 * several). Served with range requests, so a `<video>` can seek in it.
 */
export function recordingFileUrl(nodeId: string, sessionId: string, output: number, file: number, download = false) {
  const url = `/api/v1/nodes/${encodeURIComponent(nodeId)}/recordings/${encodeURIComponent(sessionId)}/outputs/${output}/files/${file}`
  return download ? `${url}?download=1` : url
}

/**
 * A failed request's message for display. The server replies with plain-text
 * error bodies; ofetch's own `message` is only "[POST] /url: 500 …".
 */
export function errorMessage(e: unknown, fallback: string): string {
  if (e instanceof FetchError && typeof e.data === 'string' && e.data.trim()) return e.data
  return e instanceof Error ? e.message : fallback
}
