<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from 'vue'
import { useRoute } from 'vue-router'
import { useEventListener } from '@vueuse/core'
import { RefreshCw } from '@lucide/vue'
import { useChecksStore } from '@/stores/checks'
import { useNodesStore } from '@/stores/nodes'
import { Button } from '@/components/ui/button'
import PageHeader from '@/components/common/PageHeader.vue'
import NodeChecks from '@/components/checks/NodeChecks.vue'

const nodes = useNodesStore()
const checks = useChecksStore()
const route = useRoute()

const problems = computed(() => nodes.reachable.reduce((n, node) => n + checks.problems(node.id), 0))
const unreachable = computed(() => nodes.nodes.filter((n) => !n.healthy))

const loading = ref(false)

async function recheck() {
  if (loading.value) return
  loading.value = true
  try {
    await checks.load()
  } finally {
    loading.value = false
  }
}

onMounted(async () => {
  await recheck()
  // From a node card's badge: scroll to that node.
  await nextTick()
  if (route.hash) document.querySelector(route.hash)?.scrollIntoView({ block: 'start' })
})

// Back from System Settings: what was just allowed shows without a click.
useEventListener(window, 'focus', recheck)
</script>

<template>
  <PageHeader title="Checks" :count="problems ? `${problems} ${problems === 1 ? 'problem' : 'problems'}` : undefined">
    <template #actions>
      <Button size="sm" variant="outline" class="h-7 gap-1.5 text-xs" :disabled="loading" @click="recheck">
        <RefreshCw class="size-3.5" :class="{ 'animate-spin': loading }" /> Check again
      </Button>
    </template>
  </PageHeader>

  <div class="p-4 space-y-3 max-w-4xl">
    <p class="text-xs text-muted-foreground">
      What each feature needs from the machines, read live from each one. Problems are things a
      source on that node needs now; the rest only matter once you use the feature.
      <template v-if="!nodes.isController">Turn on Controller (Nodes) to also test each node's ports from outside.</template>
    </p>
    <NodeChecks v-for="node in nodes.reachable" :key="node.id" :node="node" />
    <p v-if="unreachable.length" class="text-xs text-muted-foreground">
      Not checked while unreachable: {{ unreachable.map((n) => n.name).join(', ') }}.
    </p>
  </div>
</template>
