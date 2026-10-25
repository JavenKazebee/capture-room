<script setup lang="ts">
import { ref } from 'vue'
import { KeyRound } from '@lucide/vue'
import { errorMessage } from '@/composables/useApi'
import { signIn } from '@/composables/useAuth'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'

const token = ref('')
const busy = ref(false)
const error = ref('')

async function submit() {
  if (!token.value.trim() || busy.value) return
  busy.value = true
  error.value = ''
  try {
    await signIn(token.value.trim())
  } catch (e) {
    error.value = errorMessage(e, "Couldn't sign in")
    busy.value = false
  }
}
</script>

<template>
  <div class="h-svh grid place-items-center bg-background text-foreground p-4">
    <form class="w-full max-w-sm rounded-lg border border-border bg-card p-6 space-y-4" @submit.prevent="submit">
      <div class="space-y-1.5">
        <div class="flex items-center gap-2">
          <KeyRound class="size-4 text-muted-foreground" />
          <h1 class="text-base font-semibold tracking-tight">Capture Room</h1>
        </div>
        <p class="text-xs text-muted-foreground">
          This node needs its access token. On its own Mac, use Open Capture Room in the menu bar instead. Elsewhere,
          get the token from Copy Access Token in its menu bar, or Settings on a browser that's signed in; it's the
          same on every node of a controller.
        </p>
      </div>
      <Input
        v-model="token"
        type="password"
        autocomplete="current-password"
        placeholder="Access token"
        class="font-mono"
        :disabled="busy"
        autofocus
      />
      <p v-if="error" class="text-xs text-destructive">{{ error }}</p>
      <Button type="submit" class="w-full" :disabled="!token.trim() || busy">
        {{ busy ? 'Signing in…' : 'Sign in' }}
      </Button>
    </form>
  </div>
</template>
