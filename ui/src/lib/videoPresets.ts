/** Sizes and rates offered wherever a source's format is picked. */

export const RESOLUTIONS = [
  { w: 1920, h: 1080, label: '1080p' },
  { w: 1280, h: 720, label: '720p' },
  { w: 3840, h: 2160, label: '4K UHD' },
  { w: 720, h: 576, label: 'SD PAL' },
  { w: 720, h: 486, label: 'SD NTSC' },
]

export const RESOLUTION_OPTIONS = RESOLUTIONS.map((r) => ({
  value: `${r.w}x${r.h}`,
  label: `${r.label} (${r.w}×${r.h})`,
}))

export const FRAMERATES = [
  { n: 25, d: 1, label: '25 fps' },
  { n: 30, d: 1, label: '30 fps' },
  { n: 50, d: 1, label: '50 fps' },
  { n: 60, d: 1, label: '60 fps' },
  { n: 24000, d: 1001, label: '23.976 fps' },
  { n: 30000, d: 1001, label: '29.97 fps' },
  { n: 60000, d: 1001, label: '59.94 fps' },
]

export const FRAMERATE_OPTIONS = FRAMERATES.map((r) => ({ value: `${r.n}/${r.d}`, label: r.label }))
