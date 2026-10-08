/**
 * Stream URL rules, mirroring the node's (`sources/stream.rs`): SRT listens
 * when the URL has no host or says `mode=listener`. Shared by stream
 * sources and a channel's SRT outputs.
 */

export function scheme(u: string): string {
  return u.trim().match(/^([a-z][a-z0-9+.-]*):\/\//i)?.[1]?.toLowerCase() ?? ''
}

/** Host and port; the host is empty for `srt://:9000` and `udp://@:5000`. */
export function hostPort(u: string): { host: string; port: string } {
  const rest = u.split('://')[1] ?? ''
  const auth = rest.split(/[/?]/)[0].split('@').pop()!.replace(/^@/, '')
  const i = auth.lastIndexOf(':')
  return i >= 0 ? { host: auth.slice(0, i), port: auth.slice(i + 1) } : { host: auth, port: '' }
}

export function param(u: string, key: string): string | null {
  return new URLSearchParams(u.split('?')[1] ?? '').get(key)
}

export function withParam(u: string, key: string, value: string | null): string {
  const [base, query = ''] = u.split('?')
  const params = new URLSearchParams(query)
  if (value) params.set(key, value)
  else params.delete(key)
  const q = params.toString()
  return q ? `${base}?${q}` : base
}

/** Whether an SRT URL listens rather than calls. */
export function srtListens(u: string): boolean {
  const url = u.trim()
  return scheme(url) === 'srt' && (!hostPort(url).host || param(url, 'mode') === 'listener')
}

/** The node's RTSP server port (`pipeline/rtsp.rs`). */
export const RTSP_PORT = 8554

/**
 * Where an RTSP output is served: `path`, or the channel's name, made
 * URL-safe the way the node does it (`rtsp::mount_path`).
 */
export function mountPath(path: string | null, channelName: string): string {
  const wanted = path && path.trim().replace(/^\/+|\/+$/g, '') ? path.trim() : channelName
  const segments = wanted
    .split('/')
    .map((segment) => {
      let out = ''
      for (const c of segment.trim().toLowerCase()) {
        if (/[a-z0-9_.]/.test(c)) out += c
        else if (!out.endsWith('-')) out += '-'
      }
      return out.replace(/^-+|-+$/g, '')
    })
    .filter(Boolean)
  return segments.length ? `/${segments.join('/')}` : '/channel'
}
