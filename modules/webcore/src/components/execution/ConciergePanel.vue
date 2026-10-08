<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { gateway, type ChatMessage } from '@/core'
import type { ConciergeState } from '@/core/concierge'
import type { OversightProposal as Proposal, ProposalTarget, ProposalVersion } from '@/core/oversight'
import { useWorkspaceStore } from '@/stores/workspace'
import { useExecutionStore } from '@/stores/execution'
import ChatThread from '@/components/chat/ChatThread.vue'
import ChatComposer from '@/components/chat/ChatComposer.vue'
import OversightProposal from './OversightProposal.vue'
const props = defineProps<{ runId: string | null; connected: boolean; tab?: string }>()
const emit = defineEmits<{ target: [target: ProposalTarget]; version: [target: ProposalVersion] }>()
const workspace = useWorkspaceStore(), execution = useExecutionStore()
const state = ref<ConciergeState | null>(null), error = ref(''), busy = ref(false)
const composer = ref<InstanceType<typeof ChatComposer>>()
let revision = 0
let generation = 0, disposed = false, timer: ReturnType<typeof setTimeout> | undefined
let controller: AbortController | undefined
let uncertain: { id: string; text: string } | undefined
watch([() => workspace.active?.path, () => props.runId, () => props.connected], () => {
  const ticket = ++generation
  clearTimeout(timer); controller?.abort(); state.value = null; error.value = ''; busy.value = false; uncertain = undefined
  const ws = workspace.active?.path, run = props.runId
  if (!ws || !run || !props.connected) return
  async function refresh() {
    const version = revision
    try {
      const result = await gateway.getConciergeState(ws!, run!, run!)
      if (disposed || ticket !== generation) return
      if (!result.ok) throw new Error(result.error)
      if (result.data.run_id !== run || result.data.conversation_id !== run) throw new Error('Concierge identity does not match this run.')
      if (version === revision) state.value = result.data
    } catch (e) { if (!disposed && ticket === generation) { if (version === revision) { state.value = null; error.value = String(e) } } }
    finally { if (!disposed && ticket === generation) timer = setTimeout(refresh, 1000) }
  }
  void refresh()
}, { immediate: true })
onBeforeUnmount(() => { disposed = true; generation++; clearTimeout(timer); controller?.abort() })
const active = computed(() => props.connected && execution.active && execution.runId === props.runId && !state.value?.read_only)
const available = computed(() => active.value && state.value?.available === true)
const reason = computed(() => !props.runId ? 'Select a recorded run.' : !props.connected ? 'Disconnected. Receipt and execution state are unknown.' : state.value?.reason || (!state.value ? error.value ? 'Concierge state is unavailable.' : 'Loading concierge state…' : ''))
const messages = computed<ChatMessage[]>(() => [...(state.value?.messages ?? []).flatMap(turn => [
  { id: `${turn.id}:user`, role: 'user', content: turn.original_text, createdAt: turn.at_ms },
  { id: turn.id, role: turn.state === 'failed' ? 'error' : turn.state === 'answered' ? 'assistant' : 'notice', content: turn.answer || turn.error || 'Processing — no request has been confirmed.', createdAt: turn.at_ms },
]), ...(state.value?.reports ?? []).filter(r => r.status === 'completed').map(r => ({ id: `review:${r.review_id}`, role: 'assistant' as const, content: `Supervisor review · ${r.verdict}\n${r.work.answers.join('\n') || r.summary}`, createdAt: r.finished_at ?? 0 }))])
async function send(text: string) {
  const ws = workspace.active?.path, run = props.runId, ticket = generation
  if (!ws || !run || !available.value || busy.value || state.value?.messages.some(t => t.state === 'processing')) { composer.value?.setDraft(text); return }
  const id = uncertain?.text === text ? uncertain.id : crypto.randomUUID()
  revision++
  uncertain = { id, text }; busy.value = true; error.value = ''; controller = new AbortController()
  let terminal = false
  const result = await gateway.sendConciergeMessage(ws, run, run, id, text, event => {
    if (disposed || ticket !== generation || event.runId !== run || event.messageId !== id) return
    if (event.turn) {
      revision++
      terminal = event.turn.state !== 'processing'
      const current = state.value
      if (current) current.messages = [...current.messages.filter(t => t.id !== id), event.turn]
      if (event.turn.error) error.value = event.turn.error
    }
  }, controller.signal)
  if (disposed || ticket !== generation) return
  busy.value = false
  if (!result.ok || !terminal) {
    error.value = result.ok ? 'Result is not confirmed. Inspect the recorded message before retrying.' : result.error
    composer.value?.setDraft(text)
  } else uncertain = undefined
}
async function urgent(text: string) {
  const ws = workspace.active?.path, ticket = generation
  if (!ws || !active.value) { composer.value?.setDraft(text); return }
  const result = await gateway.sendInterrupt(ws, text, 'Urgent')
  if (ticket !== generation) return
  if (!result.ok) { error.value = result.error; composer.value?.setDraft(text) }
}
const noop = () => {}
const controls = computed(() => ({ files: [], pendingFiles: [], addons: [], todos: [], models: {}, modelKeys: [], model: '', effort: 'medium' as const,
  contextStats: null, usage: null, permissionMode: 'ask' as const, running: false, ready: active.value,
  concierge: { placeholder: available.value ? 'Ask about this run or send a request' : reason.value, sendDisabled: !available.value || busy.value || state.value?.messages.some(t => t.state === 'processing') === true },
  onSend: send, onQueue: send, onSteer: urgent, onStop: () => execution.cancel(), onRemoveFile: noop, onAttach: noop, onAttachPlan: noop,
  onOpenPlugins: noop, onSelectModel: noop, onSelectEffort: noop, onSelectPermission: noop, onOpenParameters: noop, onOpenSettings: noop,
}))
const labels: Record<string, string> = { received: 'Received · not processed', reviewing: 'Reviewing', awaiting_confirmation: 'Awaiting your confirmation', approved_pending_apply: 'Confirmed · not applied', applied: 'Applied', answered: 'Answered', rejected: 'Rejected', failed: 'Failed', closed_unhandled: 'Closed · unhandled' }
function status(value: string) { return labels[value] ?? `Unknown status: ${value}` }
function proposalDetails(request: NonNullable<ConciergeState['requests']>[number], item: Proposal): Proposal {
  return state.value?.reports?.filter(r => r.run_id === props.runId && r.review_id === request.review_id)
    .flatMap(r => r.proposals ?? []).find(p => p.proposal_id === item.proposal_id && p.run_id === props.runId && p.source_request_ids?.includes(request.request_id)) ?? item
}
</script>
<template>
  <section class="concierge-panel" aria-label="Run concierge">
    <p v-if="error" role="alert" class="concierge-status">{{ error }}</p>
    <p v-if="reason" role="status" class="concierge-status">{{ reason }}</p>
    <template v-if="!tab || tab === 'Chat'">
      <ChatThread :messages="messages" read-only :on-open-file="noop" :on-copy="text => { void navigator.clipboard.writeText(text) }" :on-edit="noop" :on-retry="noop" :on-restore="noop" />
      <div class="concierge-composer"><ChatComposer ref="composer" v-bind="controls" /></div>
    </template>
    <div v-else-if="tab === 'Requests'" class="request-list">
      <p v-if="!state">Request state is unavailable.</p>
      <p v-else-if="!state.requests.length">No requests recorded.</p>
      <article v-for="request in state?.requests" :key="request.request_id" class="request-card">
        <header><strong>{{ status(request.state) }}</strong><span>Concierge request</span></header>
        <p class="request-original">{{ request.original_text }}</p>
        <p v-if="request.concierge_note"><span>Model summary:</span> {{ request.concierge_note }}</p>
        <p v-if="request.state === 'received' && !state?.consumer_enabled">Supervisor processing is not enabled yet.</p>
        <p v-if="request.state === 'awaiting_confirmation'">Review the specific action in the independent approval dialog. This conversation does not grant permission.</p>
        <OversightProposal v-for="proposal in request.proposals" :key="proposal.proposal_id" :proposal="proposalDetails(request, proposal)" :run-id="runId" @target="emit('target', $event)" @version="emit('version', $event)" />
        <details><summary>Evidence</summary><p>Request: {{ request.request_id }}</p><p>Source: {{ request.source }}</p><p v-if="request.review_id">Review: {{ request.review_id }}</p><p v-for="ref in request.result_refs" :key="ref">{{ ref }}</p></details>
      </article>
    </div>
    <p v-else class="concierge-status">Supervisor reviews are not enabled yet.</p>
  </section>
</template>
<style scoped>
.concierge-panel{display:flex;flex-direction:column;flex:1;min-height:0;overflow:hidden;font-size:15px}.concierge-status{padding:12px 18px;color:var(--muted-foreground);font-size:12px;line-height:1.6}.concierge-status[role=alert]{color:var(--status-error)}.concierge-panel :deep(.chat-thread-inner){padding:18px;max-width:920px}.concierge-composer{padding:12px 14px;border-top:1px solid var(--divider)}.request-list{padding:16px;overflow:auto;flex:1}.request-card{border:1px solid var(--border);border-radius:10px;padding:14px;margin-bottom:12px;background:linear-gradient(125deg,var(--primary-soft),transparent 75%);font-size:13px;line-height:1.6;overflow-wrap:anywhere}.request-card header{display:flex;flex-direction:column;gap:3px}.request-card header span,.request-card p span{color:var(--muted-foreground);font-size:11px}.request-card p{margin:8px 0;white-space:pre-wrap}.request-card details{margin-top:10px;color:var(--muted-foreground);font-size:11px}.request-original{color:var(--foreground)}
</style>
