<script setup lang="ts">
import { RouterLink, useRoute } from 'vue-router'
import { Button } from '@/components/ui/button'

const route = useRoute()
</script>

<template>
  <div class="h-full flex flex-col items-center justify-center gap-3 p-6 text-center">
    <h1 class="text-lg font-semibold">Page not found</h1>
    <p class="text-sm text-muted-foreground break-all">Nothing is at {{ route.fullPath }}.</p>
    <Button as-child size="sm" variant="outline">
      <RouterLink to="/record">Go to Record</RouterLink>
    </Button>
  </div>
</template>
