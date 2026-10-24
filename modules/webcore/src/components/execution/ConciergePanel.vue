<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { gateway, type ChatMessage } from '@/core'
import type { ConciergeState } from '@/core/concierge'
import type {
  OversightProposal as Proposal,
  ProposalTarget,
  ProposalVersion,
} from '@/core/oversight'
import { useWorkspaceStore } from '@/stores/workspace'
import { useExecutionStore } from '@/stores/execution'
import ChatThread from '@/components/chat/ChatThread.vue'
import ChatComposer from '@/components/chat/ChatComposer.vue'
import OversightProposal from './OversightProposal.vue'
import { useWorkspaceRefresh } from '@/lib/workspace-refresh'
import { useSupervisionInputsStore } from '@/stores/supervision-inputs'
import { useFeedbackStore } from '@/stores/feedback'
import RecoveryCopies from '@/components/RecoveryCopies.vue'
import type { RecoveryRecord } from '@/core/recovery'

const props = defineProps<{ runId: string | null; connected: boolean; tab?: string }>()
const emit = defineEmits<{ target: [target: ProposalTarget]; version: [target: ProposalVersion] }>()
const workspace = useWorkspaceStore(),
  execution = useExecutionStore()
const state = ref<ConciergeState | null>(null),
  error = ref(''),
  busy = ref(false)
let revision = 0
let generation = 0,
  disposed = false
let controller: AbortController | undefined
const inputs = useSupervisionInputsStore(),
  feedback = useFeedbackStore(),
  recoveryOpen = ref(false)
const input = computed(() =>
  workspace.active?.path && props.runId ? inputs.read(workspace.active.path, props.runId) : null,
)
const draft = computed({
  get: () => input.value?.input.raw ?? '',
  set: (raw: string) => input.value?.controller.edit({ raw }),
})
const backupLabel = computed(() =>
  input.value?.state === 'unknown'
    ? 'Input unconfirmed'
    : input.value?.error
      ? 'Input kept'
      : input.value?.backup?.state === 'backed_up'
        ? 'Backed up on host'
        : draft.value
          ? 'Not backed up'
          : '',
)
async function restoreInput(record: RecoveryRecord) {
  const ws = workspace.active?.path,
    run = props.runId
  if (!ws || !run) return
  if (await inputs.restore(ws, run, record)) {
    if (workspace.active?.path === ws && props.runId === run) recoveryOpen.value = false
  } else
    feedback.toast('error', 'Input Kept', 'Back up or reconcile this draft before replacing it.')
}
async function reconcileInput() {
  await input.value?.controller.reconcile()
  void updates.run(refresh)
}
async function refresh() {
  const ws = workspace.active?.path,
    run = props.runId,
    ticket = generation,
    version = revision
  if (!ws || !run || !props.connected) return
  try {
    const result = await gateway.getConciergeState(ws, run, run)
    if (disposed || ticket !== generation || version !== revision) return
    if (!result.ok) throw new Error(result.error)
    if (result.data.run_id !== run || result.data.conversation_id !== run)
      throw new Error('Supervision identity could not be verified.')
    state.value = result.data
    error.value = ''
  } catch (reason) {
    if (!disposed && ticket === generation && version === revision) {
      state.value = null
      error.value = String(reason)
    }
  }
}
const updates = useWorkspaceRefresh(refresh, [
  () => props.runId,
  () => props.connected,
  () => props.tab,
])
watch(
  [() => workspace.active?.path, () => props.runId, () => props.connected],
  () => {
    generation++
    controller?.abort()
    state.value = null
    error.value = ''
    busy.value = false
    recoveryOpen.value = false
  },
  { immediate: true },
)
onBeforeUnmount(() => {
  disposed = true
  generation++
  controller?.abort()
})
const active = computed(
  () =>
    props.connected &&
    execution.active &&
    execution.runId === props.runId &&
    !state.value?.read_only,
)
const available = computed(() => active.value && state.value?.available === true)
const reason = computed(() =>
  !props.runId
    ? 'Select a recorded run.'
    : !props.connected
      ? 'Disconnected. Receipt and execution state are unknown.'
      : state.value?.reason ||
        (!state.value
          ? error.value
            ? 'Concierge state is unavailable.'
            : 'Loading concierge state…'
          : ''),
)
const messages = computed<ChatMessage[]>(() => [
  ...(state.value?.messages ?? []).flatMap<ChatMessage>((turn) => [
    { id: `${turn.id}:user`, role: 'user', content: turn.original_text, createdAt: turn.at_ms },
    {
      id: turn.id,
      role: turn.state === 'failed' ? 'error' : turn.state === 'answered' ? 'assistant' : 'notice',
      content: turn.answer || turn.error || 'Processing — no request has been confirmed.',
      createdAt: turn.at_ms,
    },
  ]),
  ...(state.value?.reports ?? [])
    .filter((r) => r.status === 'completed')
    .map((r) => ({
      id: `review:${r.review_id}`,
      role: 'assistant' as const,
      content: `Supervisor review · ${r.verdict}\n${r.work.answers.join('\n') || r.summary}`,
      createdAt: r.finished_at ?? 0,
    })),
])
async function send(text: string, accepted?: () => void): Promise<boolean> {
  const ws = workspace.active?.path,
    run = props.runId,
    ticket = generation,
    entry = input.value?.controller
  if (
    !ws ||
    !run ||
    !entry ||
    !available.value ||
    busy.value ||
    state.value?.messages.some((turn) => turn.state === 'processing')
  )
    return false
  const current = () =>
    !disposed && ticket === generation && workspace.active?.path === ws && props.runId === run
  busy.value = true
  error.value = ''
  revision++
  const abort = new AbortController()
  controller = abort
  const execute = async (
    id: string,
    body: string,
    receive: (turn: NonNullable<ConciergeState['messages']>[number]) => void,
  ) =>
    gateway.sendConciergeMessage(
      ws,
      run,
      run,
      id,
      body,
      (event) => {
        if (event.runId !== run || event.messageId !== id || !event.turn) return
        receive(event.turn)
        if (!current()) return
        accepted?.()
        revision++
        const value = state.value
        if (value) value.messages = [...value.messages.filter((turn) => turn.id !== id), event.turn]
        if (event.turn.error) error.value = event.turn.error
      },
      abort.signal,
    )
  try {
    if (gateway.demo) {
      const source = entry.capture(),
        result = await execute(crypto.randomUUID(), text, () => {
          if (
            entry.primary.input.edit_sequence === source.edit_sequence &&
            entry.primary.input.raw === source.raw
          )
            entry.edit({ raw: '' })
        })
      return result.ok
    }
    return await entry.submit(text, execute, current)
  } finally {
    if (current()) {
      busy.value = false
      void updates.run(refresh)
    }
  }
}
async function urgent(text: string) {
  const ws = workspace.active?.path,
    run = props.runId,
    ticket = generation,
    entry = input.value?.controller
  if (!ws || !run || !entry || !active.value) return false
  const current = () =>
    !disposed && ticket === generation && workspace.active?.path === ws && props.runId === run
  const execute = (body: string, id?: string) =>
    gateway.sendInterrupt(ws, body, 'Urgent', { kind: 'blueprint', runId: run }, id)
  if (gateway.demo) {
    const source = entry.capture(),
      result = await execute(text)
    if (result.ok && entry.primary.input.edit_sequence === source.edit_sequence)
      entry.edit({ raw: '' })
    if (current() && !result.ok) error.value = result.error
    return result.ok
  }
  const ok = await entry.temporary(execute, current)
  if (current() && !ok) error.value = entry.error
  return ok
}
async function copy(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text)
  } catch {
    error.value = 'Could not copy to the clipboard.'
  }
}
const noop = () => {}
const controls = computed(() => ({
  files: [],
  pendingFiles: [],
  addons: [],
  todos: [],
  models: {},
  modelKeys: [],
  model: '',
  effort: 'medium' as const,
  draftIdentity: `${workspace.active?.path}:${props.runId}`,
  draftValue: draft.value,
  onDraftChange: (raw: string) => (draft.value = raw),
  sendBlocked: !input.value?.canSubmit,

  readOnly: !props.runId || state.value?.read_only === true,
  contextStats: null,
  usage: null,
  permissionMode: 'ask' as const,
  running: false,
  ready: active.value,
  concierge: {
    placeholder: available.value ? 'Ask about this run or send a request' : reason.value,
    sendDisabled:
      !available.value ||
      busy.value ||
      state.value?.messages.some((t) => t.state === 'processing') === true,
  },
  onSend: send,
  onQueue: send,
  onSteer: urgent,
  onStop: () => execution.cancel(),
  onRemoveFile: noop,
  onAttach: noop,
  onAttachPlan: noop,
  onOpenPlugins: noop,
  onSelectModel: noop,
  onSelectEffort: noop,
  onSelectPermission: noop,
  onOpenParameters: noop,
  onOpenSettings: noop,
}))
const labels: Record<string, string> = {
  received: 'Received · not processed',
  reviewing: 'Reviewing',
  awaiting_confirmation: 'Awaiting your confirmation',
  approved_pending_apply: 'Confirmed · not applied',
  applied: 'Applied',
  answered: 'Answered',
  rejected: 'Rejected',
  failed: 'Failed',
  closed_unhandled: 'Closed · unhandled',
}
function status(value: string) {
  return labels[value] ?? `Unknown status: ${value}`
}
function proposalDetails(
  request: NonNullable<ConciergeState['requests']>[number],
  item: Proposal,
): Proposal {
  return (
    state.value?.reports
      ?.filter((r) => r.run_id === props.runId && r.review_id === request.review_id)
      .flatMap((r) => r.proposals ?? [])
      .find(
        (p) =>
          p.proposal_id === item.proposal_id &&
          p.run_id === props.runId &&
          p.source_request_ids?.includes(request.request_id),
      ) ?? item
  )
}
</script>
<template>
  <section class="concierge-panel" aria-label="Run concierge">
    <p v-if="error" role="alert" class="concierge-status">{{ error }}</p>
    <p v-if="reason" role="status" class="concierge-status">{{ reason }}</p>
    <template v-if="!tab || tab === 'Chat'">
      <ChatThread
        :messages="messages"
        read-only
        :on-open-file="noop"
        :on-copy="copy"
        :on-edit="noop"
        :on-retry="noop"
        :on-restore="noop"
      />
      <div class="concierge-composer">
        <ChatComposer ref="composer" v-bind="controls" />
        <div
          class="flex items-center gap-3 pt-2 text-[13px] text-muted-foreground"
          data-supervision-draft-status
        >
          <span v-if="backupLabel" role="status">{{ backupLabel }}</span>
          <button class="cursor-pointer hover:text-foreground" @click="recoveryOpen = true">
            Recovery Copies
          </button>
          <button v-if="input?.state === 'unknown'" class="btn" @click="reconcileInput">
            Check Receipt
          </button>
          <details v-if="input?.error">
            <summary class="cursor-pointer">Details</summary>
            <p>{{ input.error }}</p>
          </details>
        </div>
      </div>
    </template>
    <RecoveryCopies
      v-if="recoveryOpen && workspace.active?.path && runId"
      :key="`${workspace.active.path}:${runId}`"
      :workspace="workspace.active.path"
      scope="project"
      :entity="runId"
      :local-copies="inputs.localCopies(workspace.active.path)"
      @close="recoveryOpen = false"
      @restore="restoreInput"
    />
    <div v-if="tab === 'Requests'" class="request-list">
      <p v-if="!state">Request state is unavailable.</p>
      <p v-else-if="!state.requests.length">No requests recorded.</p>
      <article v-for="request in state?.requests" :key="request.request_id" class="request-card">
        <header>
          <strong>{{ status(request.state) }}</strong
          ><span>Concierge request</span>
        </header>
        <p class="request-original">{{ request.original_text }}</p>
        <p v-if="request.concierge_note">
          <span>Model summary:</span> {{ request.concierge_note }}
        </p>
        <p v-if="request.state === 'received' && !state?.consumer_enabled">
          Supervisor processing is not enabled yet.
        </p>
        <p v-if="request.state === 'awaiting_confirmation'">
          Review the specific action in the independent approval dialog. This conversation does not
          grant permission.
        </p>
        <OversightProposal
          v-for="proposal in request.proposals"
          :key="proposal.proposal_id"
          :proposal="proposalDetails(request, proposal)"
          :run-id="runId"
          @target="emit('target', $event)"
          @version="emit('version', $event)"
        />
        <details>
          <summary>Evidence</summary>
          <p>Request: {{ request.request_id }}</p>
          <p>Source: {{ request.source }}</p>
          <p v-if="request.review_id">Review: {{ request.review_id }}</p>
          <p v-for="ref in request.result_refs" :key="ref">{{ ref }}</p>
        </details>
      </article>
    </div>
    <p v-else-if="tab && tab !== 'Chat'" class="concierge-status">
      Supervisor reviews are not enabled yet.
    </p>
  </section>
</template>
<style scoped>
.concierge-panel {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 0;
  overflow: hidden;
  font-size: 15px;
}
.concierge-status {
  padding: 12px 18px;
  color: var(--muted-foreground);
  font-size: 13px;
  line-height: 1.6;
}
.concierge-status[role='alert'] {
  color: var(--status-error);
}
.concierge-panel :deep(.chat-thread-inner) {
  padding: 18px;
  max-width: 920px;
}
.concierge-composer {
  padding: 12px 14px;
  border-top: 1px solid var(--divider);
}
.request-list {
  padding: 16px;
  overflow: auto;
  flex: 1;
}
.request-card {
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 14px;
  margin-bottom: 12px;
  background: linear-gradient(125deg, var(--primary-soft), transparent 75%);
  font-size: 13px;
  line-height: 1.6;
  overflow-wrap: anywhere;
}
.request-card header {
  display: flex;
  flex-direction: column;
  gap: 3px;
}
.request-card header span,
.request-card p span {
  color: var(--muted-foreground);
  font-size: 13px;
}
.request-card p {
  margin: 8px 0;
  white-space: pre-wrap;
}
.request-card details {
  margin-top: 10px;
  color: var(--muted-foreground);
  font-size: 13px;
}
.request-original {
  color: var(--foreground);
}
</style>
