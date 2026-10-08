<script setup lang="ts">
import { computed } from 'vue'
import { ShieldAlert, X } from '@lucide/vue'
import { useExecutionStore } from '@/stores/execution'
import { useChatStore } from '@/stores/chat'
import { blueprintApproval } from '@/core/blueprint-approval'

/**
 * The approval dialog, for both surfaces that can be blocked on a decision: a
 * blueprint run (the execution store) and a ReAct chat turn (the chat store).
 *
 * Both are the same question — "may this run?" — so they share one dialog
 * rather than two that drift apart. Which one is pending decides where the
 * answer goes.
 */
const execution = useExecutionStore()
const chat = useChatStore()

const request = computed(() => {
  if (execution.approval) {
    return {
      title: execution.approval.title,
      detail: execution.approval.detail,
      command: execution.approval.command,
      respond: (allow: boolean) => execution.respond(allow),
    }
  }
  const pending = chat.approval
  if (!pending) return null
  const payload = parseDetail(pending.detail)
  const plan = blueprintApproval(payload)
  if (plan) return { ...plan, respond: (allow: boolean) => void chat.respondApproval(allow) }
  const path = typeof payload.path === 'string' ? payload.path : ''
  const command = typeof payload.command === 'string' ? payload.command : ''
  const subject = command || path
  const outside = payload.outside_workspace === true
  return {
    title: command ? 'Approve shell command' : 'Approve file change',
    detail: outside
      ? `${subject} is outside the workspace, and this conversation confirms such changes.`
      : `${subject} — this conversation confirms changes before they run.`,
    command: subject,
    respond: (allow: boolean) => void chat.respondApproval(allow),
  }
})

/** Parses the approval payload, tolerating an unexpected shape. */
function parseDetail(raw: string): Record<string, unknown> {
  try {
    const parsed = JSON.parse(raw)
    return parsed && typeof parsed === 'object' ? (parsed as Record<string, unknown>) : {}
  } catch {
    return {}
  }
}
</script>

<template>
  <Teleport to="body">
    <Transition name="fade">
      <div
        v-if="request"
        class="fixed inset-0 z-50 flex items-center justify-center p-4"
        style="background: var(--overlay)"
        role="dialog"
        aria-modal="true"
      >
        <div class="glass max-h-[90vh] w-full max-w-md overflow-y-auto rounded-xl p-5">
          <div class="flex items-center justify-between">
            <div class="flex items-center gap-2 text-danger">
              <ShieldAlert class="h-5 w-5" />
              <span class="text-[13px] font-semibold">{{ request.title }}</span>
            </div>
            <button class="btn-icon" type="button" aria-label="Dismiss" @click="request.respond(false)">
              <X class="h-4 w-4" />
            </button>
          </div>

          <p class="mt-3 text-[13px] text-muted-foreground">{{ request.detail }}</p>

          <pre
            v-if="request.command"
            class="mt-3 max-h-[50vh] overflow-auto rounded-lg bg-surface-muted p-3 font-mono text-[12px] leading-5 text-foreground"
            >{{ request.command }}</pre>

          <div v-if="execution.approval && execution.active" class="mt-4 flex flex-wrap gap-2 border-t border-border pt-3">
            <button class="btn" type="button" :disabled="execution.controlBusy" @click="execution.status === 'paused' ? execution.resume() : execution.pause()">{{ execution.status === 'paused' ? 'Resume run directly' : 'Pause run directly' }}</button>
            <button class="btn btn-danger-outline" type="button" :disabled="execution.controlBusy" @click="execution.cancel()">Stop run directly</button>
          </div>
          <div class="mt-5 flex justify-end gap-2">
            <button class="btn btn-danger-outline" type="button" @click="request.respond(false)">
              Deny
            </button>
            <button class="btn btn-primary" type="button" @click="request.respond(true)">
              Allow
            </button>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.15s ease;
}

.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}

.fade-enter-active .glass {
  transform: translateY(0) scale(1);
}

.fade-enter-from .glass {
  transform: translateY(8px) scale(0.97);
}
</style>
