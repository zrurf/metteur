<script setup lang="ts">
import { useFeedbackStore } from '@/stores/feedback'
import { vDialogFocus } from '@/components/resources/dialog-focus'

const feedback = useFeedbackStore()
</script>

<template>
  <Teleport to="body">
    <Transition name="confirm">
      <div
        v-if="feedback.pending"
        class="fixed inset-0 z-1100 flex items-center justify-center p-4"
        style="background: var(--overlay)"
        @mousedown.self="feedback.settle(false)"
      >
        <div
          v-dialog-focus
          role="dialog"
          aria-modal="true"
          aria-labelledby="confirm-heading"
          @keydown.esc.stop="feedback.settle(false)"
          class="max-h-[85vh] w-full max-w-sm overflow-y-auto rounded-xl p-5"
          style="
            background: var(--popover);
            box-shadow: var(--shadow-popover);
            border: 1px solid var(--divider);
          "
        >
          <h2 id="confirm-heading" class="text-[13.5px] font-semibold text-foreground">
            {{ feedback.pending.req.header }}
          </h2>
          <p class="mt-1.5 text-[12.5px] leading-relaxed text-muted-foreground wrap-break-word">
            {{ feedback.pending.req.message }}
          </p>
          <details v-if="feedback.pending.req.details" class="mt-3 text-[13px]">
            <summary>Commands & exact dependencies</summary>
            <pre class="mt-2 max-h-60 overflow-auto whitespace-pre-wrap wrap-break-word">{{
              feedback.pending.req.details
            }}</pre>
          </details>
          <div class="mt-5 flex justify-end gap-2">
            <button class="btn" type="button" autofocus @click="feedback.settle(false)">
              {{ feedback.pending.req.rejectLabel || 'Cancel' }}
            </button>
            <button
              class="btn"
              :class="feedback.pending.req.danger ? 'btn-danger-outline' : 'btn-primary'"
              type="button"
              @click="feedback.settle(true)"
            >
              {{ feedback.pending.req.acceptLabel || 'OK' }}
            </button>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.confirm-enter-active,
.confirm-leave-active {
  transition: opacity 140ms ease;
}
.confirm-enter-from,
.confirm-leave-to {
  opacity: 0;
}
</style>
