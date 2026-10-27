import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { backoff, BASE_DELAY, MAX_DELAY, reconnectingSocket } from '@/lib/reconnectingSocket'

class FakeSocket extends EventTarget {
  static CONNECTING = 0
  static OPEN = 1
  static all: FakeSocket[] = []
  readyState = FakeSocket.CONNECTING
  binaryType = 'blob'
  sent: string[] = []
  url: string
  constructor(url: string) {
    super()
    this.url = url
    FakeSocket.all.push(this)
  }
  send(data: string) {
    this.sent.push(data)
  }
  close() {
    this.readyState = 3
    this.dispatchEvent(new Event('close'))
  }
  open() {
    this.readyState = FakeSocket.OPEN
    this.dispatchEvent(new Event('open'))
  }
  receive(data: unknown) {
    this.dispatchEvent(new MessageEvent('message', { data }))
  }
}

const latest = () => FakeSocket.all[FakeSocket.all.length - 1]!

beforeEach(() => {
  vi.useFakeTimers()
  FakeSocket.all = []
  vi.stubGlobal('WebSocket', FakeSocket)
  vi.stubGlobal('location', { protocol: 'https:', host: 'node.local:7700' })
  vi.stubGlobal('window', new EventTarget())
  vi.stubGlobal('document', Object.assign(new EventTarget(), { visibilityState: 'visible' }))
})

afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe('backoff', () => {
  it('doubles from the base delay up to the cap', () => {
    expect([0, 1, 2, 3, 10].map(backoff)).toEqual([BASE_DELAY, 2 * BASE_DELAY, 4 * BASE_DELAY, MAX_DELAY, MAX_DELAY])
  })
})

describe('reconnectingSocket', () => {
  it('connects with the page protocol', () => {
    reconnectingSocket({ path: '/ws', shouldReconnect: () => true }).connect()
    expect(latest().url).toBe('wss://node.local:7700/ws')
  })

  it('backs off between failed attempts, and starts over once open', () => {
    const onOpen = vi.fn()
    const socket = reconnectingSocket({ path: '/ws', shouldReconnect: () => true, onOpen })
    socket.connect()
    latest().close()
    expect(socket.attempt).toBe(1)
    vi.advanceTimersByTime(BASE_DELAY - 1)
    expect(FakeSocket.all).toHaveLength(1)
    vi.advanceTimersByTime(1)
    expect(FakeSocket.all).toHaveLength(2)

    latest().close()
    vi.advanceTimersByTime(2 * BASE_DELAY - 1)
    expect(FakeSocket.all).toHaveLength(2)
    vi.advanceTimersByTime(1)
    expect(FakeSocket.all).toHaveLength(3)

    latest().open()
    expect(onOpen).toHaveBeenLastCalledWith(false)
    expect(socket.attempt).toBe(0)
    latest().close()
    vi.advanceTimersByTime(BASE_DELAY)
    latest().open()
    expect(onOpen).toHaveBeenLastCalledWith(true)
  })

  it('closes on an error, and reconnects', () => {
    const onClose = vi.fn()
    reconnectingSocket({ path: '/ws', shouldReconnect: () => true, onClose }).connect()
    latest().open()
    latest().dispatchEvent(new Event('error'))
    expect(onClose).toHaveBeenCalledWith(true)
    vi.advanceTimersByTime(BASE_DELAY)
    expect(FakeSocket.all).toHaveLength(2)
  })

  it('stays closed while it should not reconnect', () => {
    let wanted = true
    const socket = reconnectingSocket({ path: '/ws', shouldReconnect: () => wanted })
    socket.connect()
    wanted = false
    latest().close()
    vi.advanceTimersByTime(MAX_DELAY)
    socket.connect()
    expect(FakeSocket.all).toHaveLength(1)
  })

  it('stops after close, and ignores the closed socket', () => {
    const onClose = vi.fn()
    const socket = reconnectingSocket({ path: '/ws', shouldReconnect: () => true, onClose })
    socket.connect()
    socket.close()
    vi.advanceTimersByTime(MAX_DELAY)
    expect(FakeSocket.all).toHaveLength(1)
    expect(onClose).not.toHaveBeenCalled()
  })

  it('skips the backoff when the window comes back', () => {
    const socket = reconnectingSocket({ path: '/ws', shouldReconnect: () => true })
    socket.connect()
    latest().close()
    window.dispatchEvent(new Event('focus'))
    expect(FakeSocket.all).toHaveLength(2)
    expect(socket.attempt).toBe(0)
    // Already opening: no second socket.
    window.dispatchEvent(new Event('online'))
    document.dispatchEvent(new Event('visibilitychange'))
    expect(FakeSocket.all).toHaveLength(2)
  })

  it('drops a malformed frame and keeps going', () => {
    const onMessage = vi.fn()
    reconnectingSocket({ path: '/ws', shouldReconnect: () => true, onMessage }).connect()
    latest().open()
    expect(() => latest().receive('{"type": "sources.')).not.toThrow()
    latest().receive('{"type":"sources.changed"}')
    expect(onMessage).toHaveBeenCalledTimes(1)
    expect(onMessage).toHaveBeenCalledWith({ type: 'sources.changed' })
  })

  it('passes binary frames through as array buffers', () => {
    const onBinary = vi.fn()
    reconnectingSocket({ path: '/ws/thumbnails', shouldReconnect: () => true, onBinary }).connect()
    expect(latest().binaryType).toBe('arraybuffer')
    const frame = new ArrayBuffer(4)
    latest().receive(frame)
    expect(onBinary).toHaveBeenCalledWith(frame)
  })

  it('sends only while open', () => {
    const socket = reconnectingSocket({ path: '/ws', shouldReconnect: () => true })
    socket.connect()
    expect(socket.send('a')).toBe(false)
    latest().open()
    expect(socket.send('b')).toBe(true)
    expect(latest().sent).toEqual(['b'])
  })
})
