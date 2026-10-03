import { toast } from 'vue-sonner'
import { errorMessage } from '@/composables/useApi'
import { useEventsStore } from '@/stores/events'

/** A failed action: toast it and keep it in the event log. */
export function notifyError(title: string, e: unknown, nodeId: string | null = null) {
  const detail = errorMessage(e, '')
  toast.error(title, { description: detail || undefined })
  useEventsStore().log('error', title, { node_id: nodeId, detail: detail || undefined })
}
