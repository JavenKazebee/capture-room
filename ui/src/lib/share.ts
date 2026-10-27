/**
 * Share `url` through the OS share sheet, or `copy` it where there's none (most desktops).
 * `cancelled` means the person closed the sheet, so nothing else should happen.
 */
export async function shareLink(
  title: string,
  url: string,
  copy: (text: string) => Promise<boolean>,
): Promise<'shared' | 'cancelled' | 'copied' | 'failed'> {
  if (navigator.share) {
    try {
      await navigator.share({ title, url })
      return 'shared'
    } catch (e) {
      if (e instanceof DOMException && e.name === 'AbortError') return 'cancelled'
      // Fall through to copying.
    }
  }
  return (await copy(url)) ? 'copied' : 'failed'
}
