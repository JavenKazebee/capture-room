/**
 * Markers dropped while recording, and their export as a Resolve marker EDL.
 *
 * A keypress marks every recording it covers with one `press_id`. Each node
 * stores its recording's marker with the timecode that recording's files carry
 * at that moment, so an export needs no clocks or time zones: it lists each
 * press once, at the timecode its recordings agree on. Resolve imports the EDL
 * onto a timeline (Timelines → Import → Timeline Markers from EDL), placing
 * each marker by record timecode, so cameras synced by timecode share them.
 */
import type { ClockTimeDto } from '@/types/generated/ClockTimeDto'
import type { MarkerColor } from '@/types/generated/MarkerColor'
import type { MarkerDto } from '@/types/generated/MarkerDto'

export const MARKER_COLORS: { value: MarkerColor; label: string; swatch: string }[] = [
  { value: 'blue', label: 'Blue', swatch: '#3b82f6' },
  { value: 'cyan', label: 'Cyan', swatch: '#06b6d4' },
  { value: 'green', label: 'Green', swatch: '#22c55e' },
  { value: 'yellow', label: 'Yellow', swatch: '#eab308' },
  { value: 'red', label: 'Red', swatch: '#ef4444' },
  { value: 'pink', label: 'Pink', swatch: '#ec4899' },
  { value: 'purple', label: 'Purple', swatch: '#a855f7' },
]

export function markerSwatch(color: MarkerColor) {
  return MARKER_COLORS.find((c) => c.value === color)?.swatch ?? '#3b82f6'
}

/**
 * When a key went down, on the clock `now` was read from: `now` was taken about
 * halfway through its request, so step back from there to the keypress. Times
 * are `performance.now()` milliseconds.
 */
export function pressTime(now: ClockTimeDto, pressedAt: number, sentAt: number, receivedAt: number): ClockTimeDto {
  const ago = (sentAt + receivedAt) / 2 - pressedAt
  return { ...now, time_us: Math.round(now.time_us - Math.max(0, ago) * 1000) }
}

/** A frame rate as stored, `num/den`. */
interface Rate {
  /** Frames counted per second of timecode: 30 at 29.97. */
  base: number
  /** Frames skipped at each minute but every tenth; 0 when not drop-frame. */
  drop: number
}

function parseRate(fps: string): Rate | null {
  const [num, den] = fps.split('/').map(Number)
  if (!num || !den) return null
  const base = Math.round(num / den)
  // 29.97 and 59.94 count drop-frame, as the node stamps them.
  const drop = den === 1001 && num % 30000 === 0 ? (num / 30000) * 2 : 0
  return { base, drop }
}

/** Frames since midnight of `tc` (`hh:mm:ss:ff`, `;` before drop-frame frames). */
export function timecodeFrames(tc: string, fps: string): number | null {
  const rate = parseRate(fps)
  const m = /^(\d{2}):(\d{2}):(\d{2})[:;.](\d{2})$/.exec(tc)
  if (!rate || !m) return null
  const [h, min, s, f] = m.slice(1).map(Number) as [number, number, number, number]
  const minutes = h * 60 + min
  const dropped = rate.drop * (minutes - Math.floor(minutes / 10))
  return (minutes * 60 + s) * rate.base + f - dropped
}

/** The timecode `frames` after midnight, as `timecodeFrames` reads it. */
export function framesTimecode(frames: number, fps: string): string {
  const rate = parseRate(fps)!
  const day = 24 * 3600 * rate.base - (rate.drop ? rate.drop * (24 * 60 - 24 * 6) : 0)
  let n = ((frames % day) + day) % day
  if (rate.drop) {
    // Add back the frame numbers skipped before `n`.
    const perTen = 600 * rate.base - 9 * rate.drop
    const perMinute = 60 * rate.base - rate.drop
    const tens = Math.floor(n / perTen)
    const rest = n % perTen
    n += 9 * rate.drop * tens + (rest > rate.drop ? rate.drop * Math.floor((rest - rate.drop) / perMinute) : 0)
  }
  const f = n % rate.base
  const s = Math.floor(n / rate.base) % 60
  const min = Math.floor(n / (rate.base * 60)) % 60
  const h = Math.floor(n / (rate.base * 3600))
  const two = (v: number) => String(v).padStart(2, '0')
  return `${two(h)}:${two(min)}:${two(s)}${rate.drop ? ';' : ':'}${two(f)}`
}

/** A recording's markers, with what names it in an export. */
export interface MarkedRecording {
  key: string
  label: string
  markers: MarkerDto[]
}

export type SkipReason = 'no timecode' | 'another frame rate' | 'another timecode'

export interface MarkerExport {
  edl: string
  /** Presses listed. */
  count: number
  fps: string | null
  /** The earliest marker's timecode. */
  first: string | null
  /** Recordings whose markers couldn't be placed, and why. */
  skipped: { label: string; reason: SkipReason }[]
}

/** Resolve reads `|` as a field break, and a marker is one line. */
function cleanNote(note: string) {
  return note.replace(/[|\r\n]+/g, ' ').replace(/\s+/g, ' ').trim()
}

function mostCommon<T>(values: T[]): T | undefined {
  const counts = new Map<T, number>()
  for (const v of values) counts.set(v, (counts.get(v) ?? 0) + 1)
  let best: T | undefined
  let n = 0
  for (const [v, c] of counts) if (c > n) [best, n] = [v, c]
  return best
}

/** Every press across `recordings` once, as a Resolve marker EDL. */
export function markerEdl(recordings: MarkedRecording[], title: string): MarkerExport {
  const skipped = new Map<string, { label: string; reason: SkipReason }>()
  const skip = (r: MarkedRecording, reason: SkipReason) => {
    if (!skipped.has(r.key)) skipped.set(r.key, { label: r.label, reason })
  }

  const withTimecode = recordings.filter((r) => {
    const ok = r.markers.some((m) => m.timecode && m.fps)
    if (!ok && r.markers.length) skip(r, 'no timecode')
    return ok
  })
  const fps = mostCommon(withTimecode.flatMap((r) => r.markers.filter((m) => m.fps).map((m) => m.fps!))) ?? null

  const presses = new Map<string, { marker: MarkerDto; recording: MarkedRecording }[]>()
  for (const r of withTimecode) {
    for (const m of r.markers) {
      if (!m.timecode || !m.fps) continue
      if (m.fps !== fps) {
        skip(r, 'another frame rate')
        continue
      }
      const list = presses.get(m.press_id) ?? []
      list.push({ marker: m, recording: r })
      presses.set(m.press_id, list)
    }
  }

  const events: { frames: number; note: string; color: MarkerColor }[] = []
  for (const list of presses.values()) {
    const timecode = mostCommon(list.map((e) => e.marker.timecode!))!
    const agreed = list.filter((e) => e.marker.timecode === timecode)
    for (const e of list) if (e.marker.timecode !== timecode) skip(e.recording, 'another timecode')
    const frames = timecodeFrames(timecode, fps!)
    if (frames === null) continue
    const notes = [...new Set(agreed.map((e) => cleanNote(e.marker.note)).filter(Boolean))]
    // A press that marked one recording among several says which.
    const only = list.length === 1 && recordings.length > 1 ? cleanNote(list[0]!.recording.label) : ''
    const note = [only, notes.join(' / ')].filter(Boolean).join(': ')
    events.push({ frames, note: note || 'Marker', color: agreed[0]!.marker.color })
  }
  events.sort((a, b) => a.frames - b.frames)

  const lines = [`TITLE: ${cleanNote(title)}`, `FCM: ${fps && parseRate(fps)?.drop ? 'DROP FRAME' : 'NON-DROP FRAME'}`, '']
  events.forEach((e, i) => {
    const tcIn = framesTimecode(e.frames, fps!)
    const tcOut = framesTimecode(e.frames + 1, fps!)
    const color = `ResolveColor${e.color[0]!.toUpperCase()}${e.color.slice(1)}`
    lines.push(`${String(i + 1).padStart(3, '0')}  001      V     C        ${tcIn} ${tcOut} ${tcIn} ${tcOut}  `)
    lines.push(` |C:${color} |M:${e.note} |D:1`)
    lines.push('')
  })

  return {
    edl: lines.join('\n'),
    count: events.length,
    fps,
    first: events.length ? framesTimecode(events[0]!.frames, fps!) : null,
    skipped: [...skipped.values()],
  }
}
