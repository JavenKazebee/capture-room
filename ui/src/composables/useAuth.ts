import { computed, ref } from 'vue'
import { ofetch } from 'ofetch'
import type { AuthStatusDto } from '@/types/generated/AuthStatusDto'

/** `null` until the first check answers. */
export const authStatus = ref<AuthStatusDto | null>(null)

/** This node has an access token and this browser hasn't signed in with it. */
export const needsSignIn = computed(() => !!authStatus.value?.required && !authStatus.value.signed_in)

/** Ask the node whether it needs a token. If it can't be reached, carry on as if not: the app shows that itself. */
export async function checkAuth() {
  try {
    authStatus.value = await ofetch<AuthStatusDto>('/api/v1/auth')
  } catch {
    authStatus.value ??= { required: false, signed_in: true }
  }
  return authStatus.value
}

/** A request was refused: the token was changed, or this browser's cookie is gone. */
export function signedOut() {
  authStatus.value = { required: true, signed_in: false }
}

/** Sign in, then reload so every store starts from scratch. Throws if the token is wrong. */
export async function signIn(token: string) {
  await ofetch('/api/v1/auth/sign-in', { method: 'POST', body: { token } })
  location.reload()
}

export async function signOut() {
  await ofetch('/api/v1/auth/sign-out', { method: 'POST' })
  location.reload()
}

/** 32 random characters for a new token (190 bits). */
export function generateToken() {
  const alphabet = 'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789'
  const bytes = crypto.getRandomValues(new Uint8Array(32))
  return Array.from(bytes, (b) => alphabet[b % alphabet.length]).join('')
}
