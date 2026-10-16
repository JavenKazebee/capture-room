import { computed, ref, type Ref } from 'vue'
import { usePlayoutStore } from '@/stores/playout'
import { useReplayStore, type ReplayBuffer, type ReplayClip } from '@/stores/replay'
import type { Source } from '@/stores/sources'
import { sourceKey } from '@/composables/useApi'
import { notifyError } from '@/lib/notify'
import type { TransportAction } from '@/types/generated/TransportAction'
import type { TransportRequest } from '@/types/generated/TransportRequest'

/** Speeds offered, slowest first; either way round. */
export const SPEEDS = [0.25, 0.5, 0.75, 1] as const

export type Angle = { source: Source; buffer: ReplayBuffer }

/** Replay's commands on a channel. Each decides what to do when its turn comes, from the state earlier ones left. */
export function useReplayControl(opts: {
  channel: Ref<Source | null>
  /** Every angle with a buffer, and the ones that play on the channel. */
  angles: Ref<Angle[]>
  usableAngles: Ref<Angle[]>
  /** The selected angle; `angleKey` is where the choice is kept. */
  angle: Ref<Angle | null>
  angleKey: Ref<string>
  clips: Ref<ReplayClip[]>
  rate: Ref<number>
  lastSecs: Ref<number>
}) {
  const { channel, angles, usableAngles, angle, angleKey, clips, rate, lastSecs } = opts
  const playout = usePlayoutStore()
  const replay = useReplayStore()

  const status = computed(() => (channel.value ? playout.status.get(channel.value.key) : undefined))

  /** The replay on the channel, if it's showing one: its angle and the moment shown. */
  const showing = computed(() => {
    const clip = status.value?.clip
    if (!clip?.replay_source_id || clip.replay_start_us == null || !channel.value) return null
    const pos = status.value?.position_ms ?? clip.in_ms
    return {
      key: `${channel.value.node_id}/${clip.replay_source_id}`,
      us: clip.replay_start_us + pos * 1000,
      rate: clip.rate,
    }
  })
  const playing = computed(() => status.value?.state === 'playing')

  const inUs = ref<number | null>(null)
  const outUs = ref<number | null>(null)

  /** Commands run in order: a key pressed while one is in flight waits rather than being lost. */
  let queue: Promise<unknown> = Promise.resolve()
  /** Commands queued or running. */
  const queued = ref(0)
  const busy = computed(() => queued.value > 0)
  function run(what: string, fn: () => Promise<unknown>) {
    queued.value++
    queue = queue
      .then(fn)
      .catch((e) => notifyError(`${what} failed`, e, channel.value?.node_id ?? null))
      .finally(() => queued.value--)
  }

  /** Put `source`'s buffer on air from `fromUs` to `toUs` (or on). */
  function playSpan(source: Source, fromUs: number, toUs: number | null, play: boolean, r = rate.value) {
    const c = channel.value
    if (!c) return Promise.resolve()
    return replay.play(c.node_id, c.id, {
      source_id: source.id,
      from_us: Math.round(fromUs),
      to_us: toUs === null ? null : Math.round(toUs),
      rate: r,
      play,
    })
  }

  /** Put `source`'s buffer on air from `fromUs`, up to the out mark if it's later. */
  function load(source: Source, fromUs: number, play: boolean, r = rate.value) {
    const to = outUs.value !== null && outUs.value > fromUs ? outUs.value : null
    return playSpan(source, fromUs, to, play, r)
  }

  /** Transport on the channel, if there is one. */
  function transport(action: TransportAction, args?: Partial<Omit<TransportRequest, 'action'>>) {
    const c = channel.value
    return c ? playout.transport(c.node_id, c.id, action, args) : Promise.resolve()
  }

  /** Show a moment of the current angle, paused. */
  function pick(us: number) {
    const a = angle.value
    if (a) run('Cueing', () => load(a.source, us, false))
  }

  /** The classic: the last few seconds again, at the chosen speed. */
  function replayLast() {
    const a = angle.value
    if (!a) return
    // From the moment the key was pressed.
    const from = replay.nowUs(a.buffer) - Math.max(1, Number(lastSecs.value) || 8) * 1_000_000
    outUs.value = null
    run('Replaying', () => load(a.source, from, true))
  }

  function playMarked() {
    const a = angle.value
    if (!a) return
    run('Playing the marked clip', async () => {
      if (inUs.value !== null) await load(a.source, inUs.value, true)
    })
  }

  /** The saved clip on the channel, if it's showing one. */
  function savedOnAir() {
    const id = status.value?.clip?.replay_clip_id
    return id ? (clips.value.find((x) => x.id === id) ?? null) : null
  }

  /** The clip of `sourceId` in the group of the saved clip on air, if there's one. */
  function sibling(sourceId: string) {
    const on = savedOnAir()
    if (!on?.group_id) return null
    return clips.value.find((x) => x.group_id === on.group_id && x.source_id === sourceId && !x.missing) ?? null
  }

  /** Switch angle; a replay on air continues at the same moment (from the group's clip if one is on air, else the buffer). */
  function selectAngle(source: Source) {
    if (!usableAngles.value.some((a) => a.source.key === source.key)) return
    angleKey.value = source.key
    run(`Switching to ${source.display_name}`, async () => {
      const s = showing.value
      const c = channel.value
      if (!s || s.key === source.key || !c) return
      const clip = sibling(source.id)
      if (clip) await replay.playClip(c.id, clip, s.rate, playing.value, s.us)
      else await load(source, s.us, playing.value, s.rate)
    })
  }

  /** Play from the frame shown at the chosen speed; with nothing shown, the last few seconds. */
  function play() {
    const c = channel.value
    if (!c) return
    if (!showing.value && !busy.value) return replayLast()
    run('Playing', async () => {
      const s = showing.value
      const clip = status.value?.clip
      if (!s || !clip || playing.value) return
      if (status.value?.state === 'ended') {
        // Again from its in point (its out point backwards).
        const saved = clips.value.find((x) => x.id === clip.replay_clip_id)
        if (saved) return replay.playClip(c.id, saved, rate.value, true)
        const a = angles.value.find((x) => x.source.key === s.key)
        if (!a) return
        const start = clip.replay_start_us! + clip.in_ms * 1000
        const end = clip.out_ms == null ? null : clip.replay_start_us! + clip.out_ms * 1000
        return playSpan(a.source, start, end, true)
      }
      if (s.rate !== rate.value) await transport('speed', { rate: rate.value })
      await transport('play')
    })
  }

  function pause() {
    run('Pausing', async () => {
      if (playing.value) await transport('pause')
    })
  }

  function playPause() {
    // Decided now: a Space pressed while a replay is starting pauses it.
    if (playing.value || busy.value) {
      run('Pausing', async () => {
        if (playing.value) await transport('pause')
        else if (showing.value) await transport('play')
      })
    } else play()
  }

  function stop() {
    run('Stopping', async () => {
      if (status.value?.clip) await transport('stop')
    })
  }

  function step(frames: number) {
    run('Stepping', async () => {
      if (showing.value) await transport('step', { frames })
    })
  }

  function setRate(r: number) {
    rate.value = r
    run('Changing speed', async () => {
      if (showing.value && playing.value && showing.value.rate !== r) await transport('speed', { rate: r })
    })
  }

  function nudgeRate(dir: 1 | -1) {
    const i = SPEEDS.indexOf(Math.abs(rate.value) as (typeof SPEEDS)[number])
    const next = SPEEDS[Math.min(SPEEDS.length - 1, Math.max(0, (i < 0 ? 1 : i) + dir))]!
    setRate(rate.value < 0 ? -next : next)
  }

  const reverse = computed(() => rate.value < 0)
  function setReverse(on: boolean) {
    setRate(on ? -Math.abs(rate.value) : Math.abs(rate.value))
  }

  /** Save in → out as a clip of the selected angle, or of every angle on the channel's node. */
  function saveMarked(everyAngle: boolean) {
    const c = channel.value
    const a = angle.value
    if (!c || !a) return
    run('Saving the clip', async () => {
      const from = inUs.value
      const to = outUs.value
      if (from === null || to === null || to <= from) throw new Error('Mark an in point and a later out point first.')
      const span = { from_us: Math.round(from), to_us: Math.round(to), name: null }
      if (everyAngle && usableAngles.value.length > 1) {
        await replay.saveClipGroup(c.node_id, { source_ids: usableAngles.value.map((t) => t.source.id), ...span })
      } else {
        await replay.saveClip(c.node_id, { source_id: a.source.id, ...span })
      }
    })
  }

  /** Cue a saved clip on its first frame, or, if its group is on air, take over at the same moment. */
  function cueClip(clip: ReplayClip) {
    const c = channel.value
    if (!c) return
    const angle = angles.value.find((a) => a.source.key === sourceKey(clip.node_id, clip.source_id))
    run(`Cueing ${clip.name}`, async () => {
      const s = showing.value
      const on = savedOnAir()
      if (s && on && on.id !== clip.id && on.group_id && on.group_id === clip.group_id) {
        await replay.playClip(c.id, clip, s.rate, playing.value, s.us)
      } else {
        await replay.playClip(c.id, clip, rate.value, false)
      }
      if (angle) angleKey.value = angle.source.key
    })
  }

  function playClip(clip: ReplayClip) {
    const c = channel.value
    if (!c) return
    const angle = angles.value.find((a) => a.source.key === sourceKey(clip.node_id, clip.source_id))
    run(`Playing ${clip.name}`, async () => {
      await replay.playClip(c.id, clip, rate.value, true)
      if (angle) angleKey.value = angle.source.key
    })
  }

  /** Mark the frame shown, once the commands before have shown it. */
  function mark(which: 'in' | 'out') {
    run('Marking', async () => {
      const us = showing.value?.us ?? null
      if (us === null) return
      if (which === 'in') inUs.value = us
      else outUs.value = us
    })
  }

  function clearMarks() {
    inUs.value = null
    outUs.value = null
  }

  return {
    status,
    showing,
    playing,
    inUs,
    outUs,
    reverse,
    pick,
    replayLast,
    playMarked,
    selectAngle,
    play,
    pause,
    playPause,
    stop,
    step,
    setRate,
    nudgeRate,
    setReverse,
    saveMarked,
    cueClip,
    playClip,
    mark,
    clearMarks,
  }
}
