import type { DecklinkCardDto } from '@/types/generated/DecklinkCardDto'
import type { DecklinkDeviceDto } from '@/types/generated/DecklinkDeviceDto'
import type { DecklinkProfile } from '@/types/generated/DecklinkProfile'
import type { DecklinkStatusDto } from '@/types/generated/DecklinkStatusDto'

export const PROFILE_OPTIONS: { value: DecklinkProfile; label: string }[] = [
  { value: 'default', label: "As Desktop Video Setup left it" },
  { value: 'four-sub-devices-half', label: 'Four connectors, each in or out (half duplex)' },
  { value: 'two-sub-devices-full', label: 'Two sub-devices, each in and out (full duplex)' },
  { value: 'two-sub-devices-half', label: 'Two sub-devices, each in or out (half duplex)' },
  { value: 'one-sub-device-full', label: 'One sub-device, in and out (full duplex)' },
  { value: 'one-sub-device-half', label: 'One sub-device, in or out (half duplex)' },
]

/** A sub-device and the card it's on. */
export function findSubDevice(
  status: DecklinkStatusDto,
  key: string,
): { card: DecklinkCardDto; device: DecklinkDeviceDto } | null {
  for (const card of status.cards) {
    const device = card.devices.find((d) => d.key === key)
    if (device) return { card, device }
  }
  return null
}

/** Sub-devices that can capture (or play out), labelled with what uses them besides `self`. */
export function subDeviceOptions(status: DecklinkStatusDto, playout: boolean, self: string | null) {
  return status.cards.flatMap((card) =>
    card.devices
      .filter((d) => (playout ? d.playout : d.capture))
      .map((d) => {
        const others = d.used_by.filter((u) => u.source_id !== self)
        const busy = others.find((u) => u.playout === playout)
        const label = busy ? `${d.name} — used by ${busy.name}` : d.name
        return { value: d.key, label }
      }),
  )
}
