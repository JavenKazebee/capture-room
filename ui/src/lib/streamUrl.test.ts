import { describe, expect, it } from 'vitest'
import { hostPort, mountPath, param, scheme, srtListens, whepUrl, withParam } from '@/lib/streamUrl'

describe('scheme', () => {
  it('is the lower-cased scheme', () => {
    expect(scheme(' SRT://host:9000')).toBe('srt')
    expect(scheme('rtmp://a/b')).toBe('rtmp')
    expect(scheme('rtsp+tcp://a')).toBe('rtsp+tcp')
  })
  it('is empty without one', () => {
    expect(scheme('host:9000')).toBe('')
    expect(scheme('')).toBe('')
  })
})

describe('hostPort', () => {
  it('splits host and port', () => {
    expect(hostPort('srt://10.0.0.5:9000?mode=caller')).toEqual({ host: '10.0.0.5', port: '9000' })
    expect(hostPort('rtmp://live.example.com/app/key')).toEqual({ host: 'live.example.com', port: '' })
  })
  it('drops credentials', () => {
    expect(hostPort('rtsp://user:pw@cam.local:554/stream')).toEqual({ host: 'cam.local', port: '554' })
  })
  it('has an empty host for listeners', () => {
    expect(hostPort('srt://:9000')).toEqual({ host: '', port: '9000' })
    expect(hostPort('udp://@:5000')).toEqual({ host: '', port: '5000' })
  })
  it('is empty without a scheme', () => {
    expect(hostPort('nothing')).toEqual({ host: '', port: '' })
  })
})

describe('param and withParam', () => {
  it('reads a query parameter', () => {
    expect(param('srt://h:1?mode=listener&latency=200', 'latency')).toBe('200')
    expect(param('srt://h:1', 'mode')).toBeNull()
  })
  it('sets, replaces and removes one', () => {
    expect(withParam('srt://h:1', 'mode', 'listener')).toBe('srt://h:1?mode=listener')
    expect(withParam('srt://h:1?mode=caller&latency=200', 'mode', 'listener')).toBe('srt://h:1?mode=listener&latency=200')
    expect(withParam('srt://h:1?mode=caller', 'mode', null)).toBe('srt://h:1')
    expect(withParam('srt://h:1?mode=caller&latency=200', 'mode', '')).toBe('srt://h:1?latency=200')
  })
})

describe('srtListens', () => {
  it('listens without a host or with mode=listener', () => {
    expect(srtListens('srt://:9000')).toBe(true)
    expect(srtListens(' srt://0.0.0.0:9000?mode=listener ')).toBe(true)
  })
  it('calls otherwise', () => {
    expect(srtListens('srt://10.0.0.5:9000')).toBe(false)
    expect(srtListens('srt://10.0.0.5:9000?mode=caller')).toBe(false)
  })
  it('is only for SRT', () => {
    expect(srtListens('udp://@:5000')).toBe(false)
  })
})

describe('mountPath', () => {
  it('uses the path, made URL-safe', () => {
    expect(mountPath('/Main Stage/', 'Channel 1')).toBe('/main-stage')
    expect(mountPath('live/Cam #1', 'x')).toBe('/live/cam-1')
    expect(mountPath('a//b', 'x')).toBe('/a/b')
    expect(mountPath('cam_1.hd', 'x')).toBe('/cam_1.hd')
  })
  it('falls back to the channel name', () => {
    expect(mountPath(null, 'Channel 1')).toBe('/channel-1')
    expect(mountPath('  /// ', 'Program Out')).toBe('/program-out')
  })
  it('falls back to /channel when nothing is left', () => {
    expect(mountPath(null, '!!!')).toBe('/channel')
  })
  it('builds a WHEP URL', () => {
    expect(whepUrl('https://node:7700', null, 'Program')).toBe('https://node:7700/whep/program')
  })
})
