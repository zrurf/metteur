import { defineStore } from 'pinia'
import { ref } from 'vue'

export type ToastKind = 'info' | 'success' | 'error'

export interface ToastMsg {
  id: number
  kind: ToastKind
  summary: string
  detail?: string
}

export interface ConfirmRequest {
  header: string
  message: string
  details?: string
  acceptLabel?: string
  rejectLabel?: string
  /** Stroke the accept button as destructive. */
  danger?: boolean
}

interface PendingConfirm {
  req: ConfirmRequest
  resolve: (ok: boolean) => void
}

let toastSeq = 0

/**
 * In-app toast + confirm-dialog state, rendered by `ToastHost` and
 * `ConfirmDialog`. Kept out of `App.vue` so any view can trigger feedback
 * without composing components.
 */
export const useFeedbackStore = defineStore('feedback', () => {
  const toasts = ref<ToastMsg[]>([])
  const pending = ref<PendingConfirm | null>(null)

  function toast(kind: ToastKind, summary: string, detail?: string) {
    const id = toastSeq++
    toasts.value = [...toasts.value, { id, kind, summary, detail }]
    setTimeout(() => dismiss(id), 4000)
  }

  function dismiss(id: number) {
    toasts.value = toasts.value.filter((t) => t.id !== id)
  }

  function confirm(req: ConfirmRequest): Promise<boolean> {
    if (pending.value) pending.value.resolve(false)
    return new Promise((resolve) => {
      pending.value = { req, resolve }
    })
  }

  function settle(ok: boolean) {
    pending.value?.resolve(ok)
    pending.value = null
  }

  return { toasts, pending, toast, dismiss, confirm, settle }
})
