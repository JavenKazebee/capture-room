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

/** A capture dongle's HDMI audio is its own device, usually its video's name or that plus "Audio". */
function pairedAudioInput(video: DeviceDto, devices: DeviceDto[]): DeviceDto | null {
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

/** `audio` with the device's paired audio, if audio is still unchosen (silence). */
export function withPairedAudio(audio: AudioPlan, video: DeviceDto | undefined, devices: DeviceDto[]): AudioPlan {
  if (audio.from !== 'silence' || !video) return audio
  const input = pairedAudioInput(video, devices)
  if (!input) return audio
  const count = Math.min(input.channels ?? 2, 2)
  return { from: 'device', device_key: input.key, channels: Array.from({ length: count }, (_, i) => i + 1) }
}
