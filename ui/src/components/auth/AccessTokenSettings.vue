<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { toast } from 'vue-sonner'
import { RefreshCw } from '@lucide/vue'
import { checkAuth, generateToken, signOut } from '@/composables/useAuth'
import { notifyError } from '@/lib/notify'
import { plural } from '@/lib/format'
import { useNodesStore } from '@/stores/nodes'
import { useSettingsStore } from '@/stores/settings'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import CopyButton from '@/components/common/CopyButton.vue'

const nodes = useNodesStore()
const peers = computed(() => nodes.nodes.length - 1)
const scope = computed(() => (peers.value ? `this node and its ${plural(peers.value, 'other node')}` : 'this node'))

const settings = useSettingsStore()
const current = computed(() => settings.accessToken)

async function load() {
  try {
    await settings.loadAccessToken()
  } catch (e) {
    notifyError("Couldn't load the access token", e)
  }
}
onMounted(load)

/** The token being set; `null` while the form is closed. */
const draft = ref<string | null>(null)
const saving = ref(false)

function startEdit() {
  draft.value = generateToken()
}

async function save(token: string) {
  saving.value = true
  try {
    const failed = await settings.setAccessToken(token)
    await checkAuth()
    await load()
    draft.value = null
    if (failed.length) {
      toast.warning(`Not set on ${failed.map((f) => f.name).join(', ')}`, {
        description: `${failed[0]!.error}. ${failed.length === 1 ? 'It keeps' : 'They keep'} the old token until it's paired again.`,
        duration: 15_000,
      })
    } else {
      toast.success(`Access token changed on ${scope.value}`)
    }
  } catch (e) {
    notifyError("Couldn't change the access token", e)
  } finally {
    saving.value = false
  }
}
</script>

<template>
  <!-- Linked to by its id from the command palette. -->
  <section id="access-token" class="space-y-2 scroll-mt-4">
    <div>
      <h2 class="text-base font-semibold tracking-tight">Access token</h2>
      <p class="text-xs text-muted-foreground">
        A shared secret every browser signs in with once; on the node's own Mac, Open Capture Room in the menu bar
        signs in by itself. Changed here, it's changed on {{ scope }} too. WHEP stays open; WHIP sources have tokens of their own.
      </p>
    </div>
    <div class="rounded-lg border border-border bg-card overflow-hidden">
      <div class="divide-y divide-border">
        <div class="flex items-center justify-between gap-4 px-4 py-3">
          <div class="min-w-0">
            <div class="text-sm">Token</div>
            <div class="text-xs text-muted-foreground">Copy it to sign in on another computer.</div>
          </div>
          <div v-if="draft === null" class="flex items-center gap-2 shrink-0">
            <CopyButton v-if="current" :value="current.token" label="Copy token" size="md" />
            <Button variant="outline" size="sm" class="h-7 text-xs" @click="signOut">Sign out</Button>
            <Button size="sm" class="h-7 text-xs" :disabled="saving" @click="startEdit">Change…</Button>
          </div>
        </div>
        <form v-if="draft !== null" class="px-4 py-3 space-y-2" @submit.prevent="save(draft)">
          <div class="text-xs text-muted-foreground">
            Browsers signed in with the old token need this one. Or paste the token your other nodes use.
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
              {{ saving ? 'Changing…' : 'Change token' }}
            </Button>
          </div>
        </form>
        <div class="flex items-center justify-between gap-4 px-4 py-3">
          <div class="min-w-0">
            <div class="text-sm">Pairing code</div>
            <div class="text-xs text-muted-foreground">
              What a controller adds this node with. It changes once used, and after a few wrong tries.
            </div>
          </div>
          <div class="flex items-center gap-2 shrink-0">
            <span class="num text-sm">{{ current?.pairing_code ?? '—' }}</span>
            <Button variant="ghost" size="icon" class="size-7" title="Refresh" @click="load">
              <RefreshCw class="size-3.5" />
            </Button>
          </div>
        </div>
      </div>
    </div>
  </section>
</template>
