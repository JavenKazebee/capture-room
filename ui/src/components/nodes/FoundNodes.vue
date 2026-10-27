<script setup lang="ts">
import { ref } from 'vue'
import { toast } from 'vue-sonner'
import { formatFingerprint } from '@/lib/format'
import { notifyError } from '@/lib/notify'
import { needsPairingCode, useNodesStore } from '@/stores/nodes'
import type { FoundNodeDto } from '@/types/generated/FoundNodeDto'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'

/** Nodes found over mDNS, waiting to be approved (with a pairing code if they need one) or dismissed. */
const emit = defineEmits<{ approved: [] }>()

const store = useNodesStore()

/** Found nodes with an approve or dismiss on its way. */
const deciding = ref(new Set<string>())
/** The found node asking for its pairing code, and the code typed so far. */
const codeFor = ref<string | null>(null)
const foundCode = ref('')

async function decide(node: FoundNodeDto, approve: boolean) {
  if (deciding.value.has(node.id)) return
  deciding.value.add(node.id)
  try {
    if (approve) {
      await store.approve(node.id, codeFor.value === node.id ? foundCode.value.trim() : undefined)
      codeFor.value = null
      toast.success(`Approved ${node.name}`)
      emit('approved')
    } else {
      await store.dismiss(node.id)
    }
  } catch (e) {
    if (approve && needsPairingCode(e) && codeFor.value !== node.id) {
      codeFor.value = node.id
      foundCode.value = ''
    } else {
      notifyError(`Couldn't ${approve ? 'approve' : 'dismiss'} ${node.name}`, e)
    }
  } finally {
    deciding.value.delete(node.id)
  }
}
</script>

<template>
  <section v-if="store.found.length" class="rounded-lg border border-border bg-card">
    <header class="px-3 pt-3 pb-2">
      <h2 class="text-sm font-medium">Found on the network</h2>
      <p class="text-xs text-muted-foreground">
        Approve a node to control it from here with its pairing code, from its menu bar; it gets this
        controller's access token. Check its key matches the one under Settings › About on that machine first.
      </p>
    </header>
    <ul class="divide-y divide-border border-t border-border">
      <li v-for="f in store.found" :key="f.id" class="flex items-center gap-3 px-3 py-2">
        <div class="min-w-0 flex-1">
          <div class="text-sm truncate">
            {{ f.name }} <span class="text-xs text-muted-foreground num">· {{ f.url }} · v{{ f.version }}</span>
          </div>
          <div class="text-xs text-muted-foreground num break-all">Key {{ formatFingerprint(f.fingerprint) }}</div>
        </div>
        <Button
          size="sm"
          variant="ghost"
          class="h-7 text-xs"
          :disabled="deciding.has(f.id)"
          @click="decide(f, false)"
        >
          Dismiss
        </Button>
        <form v-if="codeFor === f.id" class="contents" @submit.prevent="decide(f, true)">
          <Input
            v-model="foundCode"
            placeholder="Pairing code"
            aria-label="Pairing code"
            class="h-7 w-32 font-mono text-xs uppercase"
            maxlength="12"
            spellcheck="false"
            autocomplete="off"
            :disabled="deciding.has(f.id)"
            autofocus
          />
          <Button type="submit" size="sm" class="h-7 text-xs" :disabled="deciding.has(f.id) || !foundCode.trim()">
            Approve
          </Button>
        </form>
        <Button v-else size="sm" class="h-7 text-xs" :disabled="deciding.has(f.id)" @click="decide(f, true)">
          Approve
        </Button>
      </li>
    </ul>
  </section>
</template>
