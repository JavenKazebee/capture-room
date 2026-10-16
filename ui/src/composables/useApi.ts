import { FetchError, ofetch } from 'ofetch'

export const api = ofetch.create({ baseURL: '/api/v1' })

/** One node's API; on a controller the server forwards `/nodes/{id}/…` to peers. */
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

/** A finished session's file, served with range requests so a `<video>` can seek. */
export function recordingFileUrl(nodeId: string, sessionId: string, output: number, file: number, download = false) {
  const url = `/api/v1/nodes/${encodeURIComponent(nodeId)}/recordings/${encodeURIComponent(sessionId)}/outputs/${output}/files/${file}`
  return download ? `${url}?download=1` : url
}

/** A media library item's file, with range requests; `download` to save it. */
export function mediaFileUrl(nodeId: string, mediaId: string, download = false) {
  const url = `/api/v1/nodes/${encodeURIComponent(nodeId)}/media/${encodeURIComponent(mediaId)}/file`
  return download ? `${url}?download=1` : url
}

/** Start the browser downloading `url`, as clicking a download link would. */
export function downloadUrl(url: string) {
  const a = document.createElement('a')
  a.href = url
  a.download = ''
  document.body.append(a)
  a.click()
  a.remove()
}

/** A failed request's message: the server's plain-text body, since ofetch's `message` is only "[POST] /url: 500 …". */
export function errorMessage(e: unknown, fallback: string): string {
  if (e instanceof FetchError && typeof e.data === 'string' && e.data.trim()) return e.data
  return e instanceof Error ? e.message : fallback
}
