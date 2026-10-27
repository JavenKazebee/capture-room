/**
 * A WebSocket to this page's server that reconnects with backoff, and
 * straight away when the network, the window or the tab comes back.
 */

// The server is on the local network (or this machine): retry often, so the
// page catches up within a few seconds of it coming back.
export const BASE_DELAY = 500
export const MAX_DELAY = 3_000

/** Wait before reconnect attempt `attempt` (0 for the first after a close). */
export function backoff(attempt: number): number {
  return Math.min(BASE_DELAY * 2 ** attempt, MAX_DELAY)
}

export interface ReconnectingSocketOptions {
  /** Server path, such as `/ws`. */
  path: string
  /** Each text frame, parsed; a frame that isn't JSON is dropped. */
  onMessage?: (data: unknown) => void
  /** Each binary frame; set to receive them as `ArrayBuffer`s. */
  onBinary?: (data: ArrayBuffer) => void
  /** Each attempt, as it starts. */
  onConnecting?: () => void
  /** `reconnect`: it was open before since `connect`. */
  onOpen?: (reconnect: boolean) => void
  /** `wasOpen`: it closed after opening, rather than failing to. */
  onClose?: (wasOpen: boolean) => void
  /** Checked before every attempt, the first included: false leaves it closed. */
  shouldReconnect: () => boolean
}

export interface ReconnectingSocket {
  /** Open it unless it's open, opening or waiting out the backoff. */
  connect(): void
  /** Skip the backoff and open it now, unless it's open or opening. */
  reconnectNow(): void
  /** Close it and stop reconnecting until `connect`. */
  close(): void
  /** Send `data` if it's open; whether it was. */
  send(data: string): boolean
  /** Closes since it was last open, the one that closed it included; 0 while open. */
  readonly attempt: number
}

export function reconnectingSocket(options: ReconnectingSocketOptions): ReconnectingSocket {
  let socket: WebSocket | null = null
  let timer: ReturnType<typeof setTimeout> | null = null
  let attempt = 0
  let openedBefore = false

  function clearTimer() {
    if (timer) clearTimeout(timer)
    timer = null
  }

  function open() {
    if (socket || timer || !options.shouldReconnect()) return
    const proto = location.protocol === 'https:' ? 'wss' : 'ws'
    const ws = new WebSocket(`${proto}://${location.host}${options.path}`)
    if (options.onBinary) ws.binaryType = 'arraybuffer'
    socket = ws
    let isOpen = false
    options.onConnecting?.()

    ws.addEventListener('open', () => {
      if (ws !== socket) return
      isOpen = true
      attempt = 0
      const reconnect = openedBefore
      openedBefore = true
      options.onOpen?.(reconnect)
    })
    ws.addEventListener('message', (ev) => {
      if (ws !== socket) return
      if (typeof ev.data === 'string') {
        let data: unknown
        try {
          data = JSON.parse(ev.data)
        } catch {
          return
        }
        options.onMessage?.(data)
      } else if (ev.data instanceof ArrayBuffer) {
        options.onBinary?.(ev.data)
      }
    })
    ws.addEventListener('close', () => {
      // A socket `close` let go of, or one already replaced.
      if (ws !== socket) return
      socket = null
      const delay = backoff(attempt++)
      options.onClose?.(isOpen)
      if (!options.shouldReconnect()) return
      timer = setTimeout(() => {
        timer = null
        open()
      }, delay)
    })
    ws.addEventListener('error', () => ws.close())
  }

  function reconnectNow() {
    if (socket) return
    clearTimer()
    attempt = 0
    open()
  }

  window.addEventListener('online', reconnectNow)
  window.addEventListener('focus', reconnectNow)
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible') reconnectNow()
  })

  return {
    connect() {
      if (!socket && !timer) openedBefore = false
      open()
    },
    reconnectNow,
    close() {
      clearTimer()
      const ws = socket
      socket = null
      ws?.close()
    },
    send(data) {
      if (socket?.readyState !== WebSocket.OPEN) return false
      socket.send(data)
      return true
    },
    get attempt() {
      return attempt
    },
  }
}
