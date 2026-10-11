/**
 * A WHEP viewer: receives a stream from a WHEP endpoint (a node's WHEP
 * output, `api/whep.rs`) into a `MediaStream`.
 *
 * Non-trickle: the offer goes once ICE gathering is done, so the endpoint
 * never needs a PATCH. Leaving deletes the session.
 */
export interface WhepSession {
  stream: MediaStream
  /** Leave: closes the connection and ends the session on the node. */
  close: () => void
  pc: RTCPeerConnection
}

const GATHER_TIMEOUT_MS = 2000

export async function watchWhep(endpoint: string): Promise<WhepSession> {
  const pc = new RTCPeerConnection()
  const stream = new MediaStream()
  pc.addTransceiver('video', { direction: 'recvonly' })
  pc.addTransceiver('audio', { direction: 'recvonly' })
  pc.ontrack = (e) => stream.addTrack(e.track)

  let resource: string | null = null
  const close = () => {
    pc.close()
    if (resource) fetch(resource, { method: 'DELETE' }).catch(() => {})
    resource = null
  }

  try {
    await pc.setLocalDescription(await pc.createOffer())
    await gathered(pc)
    const resp = await fetch(endpoint, {
      method: 'POST',
      headers: { 'Content-Type': 'application/sdp' },
      body: pc.localDescription!.sdp,
    })
    if (resp.status !== 201) {
      const text = (await resp.text()).trim()
      throw new Error(
        resp.status === 503
          ? 'The output is still starting: try again in a moment.'
          : text || `The node answered ${resp.status}`,
      )
    }
    const location = resp.headers.get('Location')
    if (location) resource = new URL(location, endpoint).toString()
    await pc.setRemoteDescription({ type: 'answer', sdp: await resp.text() })
  } catch (e) {
    close()
    throw e
  }
  return { stream, close, pc }
}

/** Wait for ICE gathering to finish (or give up waiting and send what there is). */
function gathered(pc: RTCPeerConnection): Promise<void> {
  if (pc.iceGatheringState === 'complete') return Promise.resolve()
  return new Promise((resolve) => {
    const done = () => {
      pc.removeEventListener('icegatheringstatechange', check)
      clearTimeout(timer)
      resolve()
    }
    const check = () => pc.iceGatheringState === 'complete' && done()
    const timer = setTimeout(done, GATHER_TIMEOUT_MS)
    pc.addEventListener('icegatheringstatechange', check)
  })
}
