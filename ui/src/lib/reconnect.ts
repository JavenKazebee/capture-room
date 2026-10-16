import { getCurrentScope, onScopeDispose } from 'vue'

type Reload = () => unknown

const reloads = new Set<Reload>()

/**
 * Run `reload` on reconnect, since events sent while away are lost. Call it where the data is
 * first loaded; a component's registration ends when it unmounts.
 */
export function onReconnect(reload: Reload) {
  reloads.add(reload)
  if (getCurrentScope()) onScopeDispose(() => reloads.delete(reload))
}

/** Run every registered reload; failures are left to each one. */
export async function runReconnectReloads() {
  await Promise.all([...reloads].map(async (reload) => {
    try {
      await reload()
    } catch {
      // A reload that fails keeps its old data until the next event.
    }
  }))
}
