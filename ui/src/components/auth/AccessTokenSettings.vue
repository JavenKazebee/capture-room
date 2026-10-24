<script setup lang="ts">
import { computed, ref } from 'vue'
import { toast } from 'vue-sonner'
import { AlertTriangle, RefreshCw } from '@lucide/vue'
import { api } from '@/composables/useApi'
import { authStatus, checkAuth, generateToken, signOut } from '@/composables/useAuth'
import { notifyError } from '@/lib/notify'
import { plural } from '@/lib/format'
import { useNodesStore } from '@/stores/nodes'
import type { AccessTokenResultDto } from '@/types/generated/AccessTokenResultDto'
import type { AccessTokenRequest } from '@/types/generated/AccessTokenRequest'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import CopyButton from '@/components/common/CopyButton.vue'
import ConfirmDialog from '@/components/common/ConfirmDialog.vue'

const nodes = useNodesStore()
const on = computed(() => !!authStatus.value?.required)
const peers = computed(() => nodes.nodes.length - 1)
const scope = computed(() => (peers.value ? `this node and its ${plural(peers.value, 'other node')}` : 'this node'))

/** The token being set; `null` while the form is closed. */
const draft = ref<string | null>(null)
const saving = ref(false)
const confirmOff = ref(false)

function startEdit() {
  draft.value = generateToken()
}

async function save(token: string | null) {
  saving.value = true
  try {
    const { failed } = await api<AccessTokenResultDto>('/access-token', {
      method: 'PUT',
      body: { token } satisfies AccessTokenRequest,
    })
    await checkAuth()
    draft.value = null
    if (failed.length) {
      toast.warning(`Not set on ${failed.map((f) => f.name).join(', ')}`, {
        description: `${failed[0]!.error}. ${failed.length === 1 ? 'It keeps' : 'They keep'} the old token until it's set there.`,
        duration: 15_000,
      })
    } else {
      toast.success(token ? `Access token set on ${scope.value}` : `Access token turned off on ${scope.value}`)
    }
  } catch (e) {
    notifyError("Couldn't set the access token", e)
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <!-- The access token check links here by its id. -->
  <section id="access-token" class="space-y-2 scroll-mt-4">
    <div>
      <h2 class="text-base font-semibold tracking-tight">Access token</h2>
      <p class="text-xs text-muted-foreground">
        A shared secret every browser signs in with once. Set here, it's set on {{ scope }} too. WHEP and WHIP stay open.
      </p>
    </div>
    <div class="rounded-lg border border-border bg-card overflow-hidden">
      <div class="divide-y divide-border">
        <div class="flex items-center justify-between gap-4 px-4 py-3">
          <div class="min-w-0">
            <div class="text-sm">{{ on ? 'On' : 'Off' }}</div>
            <div v-if="on" class="text-xs text-muted-foreground">The API needs the token. It isn't shown again once set.</div>
            <div v-else class="text-xs text-warning inline-flex items-center gap-1.5">
              <AlertTriangle class="size-3.5 shrink-0" />
              Anyone who can reach these nodes can record, delete files or update them.
            </div>
          </div>
          <div class="flex items-center gap-2 shrink-0">
            <template v-if="draft === null">
              <Button v-if="on" variant="outline" size="sm" class="h-7 text-xs" :disabled="saving" @click="confirmOff = true">Turn off</Button>
              <Button v-if="on" variant="outline" size="sm" class="h-7 text-xs" @click="signOut">Sign out</Button>
              <Button size="sm" class="h-7 text-xs" :disabled="saving" @click="startEdit">{{ on ? 'Change…' : 'Turn on…' }}</Button>
            </template>
          </div>
        </div>
        <form v-if="draft !== null" class="px-4 py-3 space-y-2" @submit.prevent="save(draft)">
          <div class="text-xs text-muted-foreground">
            Copy it somewhere safe first: it's needed to sign in, and to add a node that already has one. Or paste the token your other nodes use.
          </div>
          <div class="flex items-center gap-2">
            <Input v-model="draft" class="h-8 font-mono text-xs" spellcheck="false" autocomplete="off" :disabled="saving" />
            <CopyButton :value="draft" label="Copy token" size="md" />
            <Button type="button" variant="ghost" size="icon" class="size-7 shrink-0" title="Generate another" :disabled="saving" @click="startEdit">
              <RefreshCw class="size-3.5" />
            </Button>
          </div>
          <div class="flex justify-end gap-2">
            <Button type="button" variant="outline" size="sm" class="h-7 text-xs" :disabled="saving" @click="draft = null">Cancel</Button>
            <Button type="submit" size="sm" class="h-7 text-xs" :disabled="saving || draft.trim().length < 16">
              {{ saving ? 'Setting…' : 'Set token' }}
            </Button>
          </div>
        </form>
      </div>
    </div>
    <ConfirmDialog
      v-model:open="confirmOff"
      title="Turn off the access token?"
      :description="`Anyone who can reach ${scope} will be able to control ${peers ? 'them' : 'it'}.`"
      confirm-label="Turn off"
      @confirm="save(null)"
    />
  </section>
</template>
