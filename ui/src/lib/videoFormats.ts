/** Sizes and rates offered wherever a format is picked: a source's, a channel's or a recording's. */

// SD is offered for sources only: a recording leg scales the source, and nobody
// downscales an HD feed to SD to record it.
const RESOLUTIONS = [
  { w: 3840, h: 2160, label: '4K UHD' },
  { w: 2560, h: 1440, label: '1440p' },
  { w: 1920, h: 1080, label: '1080p' },
  { w: 1280, h: 720, label: '720p' },
  { w: 720, h: 576, label: 'SD PAL', sd: true },
  { w: 720, h: 486, label: 'SD NTSC', sd: true },
]

const FRAMERATES = [
  { n: 24000, d: 1001, label: '23.976 fps' },
  { n: 24, d: 1, label: '24 fps' },
  { n: 25, d: 1, label: '25 fps' },
  { n: 30000, d: 1001, label: '29.97 fps' },
  { n: 30, d: 1, label: '30 fps' },
  { n: 50, d: 1, label: '50 fps' },
  { n: 60000, d: 1001, label: '59.94 fps' },
  { n: 60, d: 1, label: '60 fps' },
]

const resolutionLabel = (r: (typeof RESOLUTIONS)[number]) => `${r.label} (${r.w}×${r.h})`

/** Sizes for a source or channel, valued `WxH`. */
export const RESOLUTION_OPTIONS = RESOLUTIONS.map((r) => ({ value: `${r.w}x${r.h}`, label: resolutionLabel(r) }))

/** Rates for a source or channel, valued `N/D`. */
export const FRAMERATE_OPTIONS = FRAMERATES.map((r) => ({ value: `${r.n}/${r.d}`, label: r.label }))

/** Common recording sizes; anything else is entered as a custom WIDTHxHEIGHT. */
export const RESOLUTION_PRESETS = RESOLUTIONS.filter((r) => !r.sd).map((r) => ({
  value: `${r.w}x${r.h}`,
  label: resolutionLabel(r),
}))

/** Common recording rates, stored as the node's `parse_framerate` reads them ("25", "30000/1001"). */
export const FRAMERATE_PRESETS = FRAMERATES.map((r) => ({ value: r.d === 1 ? `${r.n}` : `${r.n}/${r.d}`, label: r.label }))

/** The listed resolution for a `WxH` option value. */
export function findResolution(key: string) {
  return RESOLUTIONS.find((r) => `${r.w}x${r.h}` === key)
}

/** The listed frame rate for an `N/D` option value. */
export function findFramerate(key: string) {
  return FRAMERATES.find((r) => `${r.n}/${r.d}` === key)
}

/** Audio channel counts offered for generated and mixed audio. */
export const AUDIO_CHANNEL_OPTIONS = [
  { value: 1, label: 'Mono' },
  { value: 2, label: 'Stereo' },
  { value: 6, label: '5.1' },
  { value: 8, label: '7.1' },
]
