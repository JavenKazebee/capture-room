import type { AudioPlan } from '@/types/generated/AudioPlan'
import type { DeviceDto } from '@/types/generated/DeviceDto'

/** A device name without the words that tell its video and audio halves apart. */
function core(name: string) {
  return name
    .toLowerCase()
    .replace(/\b(audio|video|input|microphone|mic)\b/g, '')
    .replace(/[^a-z0-9]+/g, ' ')
    .trim()
}

/**
 * The audio input that belongs to a video device, if one looks like it:
 * a capture dongle's HDMI audio is listed as its own device, usually under
 * the same name ("Cam Link 4K") or the name plus "Audio".
 */
export function pairedAudioInput(video: DeviceDto, devices: DeviceDto[]): DeviceDto | null {
  const v = core(video.name)
  if (v.length < 4) return null
  return (
    devices.find((d) => d.kind === 'audio' && core(d.name) === v) ??
    devices.find((d) => {
      if (d.kind !== 'audio') return false
      const a = core(d.name)
      return a.length >= 6 && (a.startsWith(v) || v.startsWith(a))
    }) ??
    null
  )
}

/**
 * `audio` with a video device's own audio paired, when the audio hasn't been
 * chosen yet (still silence) and the device has one.
 */
export function withPairedAudio(audio: AudioPlan, video: DeviceDto | undefined, devices: DeviceDto[]): AudioPlan {
  if (audio.from !== 'silence' || !video) return audio
  const input = pairedAudioInput(video, devices)
  if (!input) return audio
  const count = Math.min(input.channels ?? 2, 2)
  return { from: 'device', device_key: input.key, channels: Array.from({ length: count }, (_, i) => i + 1) }
}
