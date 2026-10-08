<script setup lang="ts">
import { computed, onMounted, onBeforeUnmount, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { FileCode2, Play, Pause, Square, GitBranch } from '@lucide/vue'
import { gateway, type Blueprint } from '@/core'
import { useExecutionStore } from '@/stores/execution'
import { useWorkspaceStore } from '@/stores/workspace'
import { useTabsStore } from '@/stores/tabs'
import { usePanelStore } from '@/stores/panel'
import { fileRoute } from '@/lib/file-token'
import OversightReviews from '@/components/execution/OversightReviews.vue'
import ConciergePanel from '@/components/execution/ConciergePanel.vue'
import ExecutionGraph from '@/components/execution/ExecutionGraph.vue'
import ExecutionBlackboard from '@/components/execution/ExecutionBlackboard.vue'
import ExecutionUsage from '@/components/execution/ExecutionUsage.vue'
import type { Invocation } from '@/core/execution-view'
import type { ProposalTarget, ProposalVersion } from '@/core/oversight'
import { useSurfaceNavigation } from '@/lib/surface'
import { useVersionStore } from '@/stores/version'
import ExecutionLayout from '@/components/execution/ExecutionLayout.vue'
import FilePickerDialog, { type FilePick } from '@/components/FilePickerDialog.vue'
const execution = useExecutionStore(), workspace = useWorkspaceStore(), tabs = useTabsStore()
const router = useRouter(), panel = usePanelStore()
const picker = ref(false), selected = ref<Blueprint | null>(null), path = ref(''), error = ref(''), busy = ref(false)
const graph = computed(() => execution.active ? execution.blueprint : selected.value ?? execution.blueprint)
const source = computed(() => selected.value && !execution.active ? path.value : execution.sourcePath)
const inspected = ref<Invocation | null>(null)
const proposalTarget = ref<ProposalTarget | null>(null)
const layout = ref<InstanceType<typeof ExecutionLayout>>()
function inspect(record: Invocation | null) { inspected.value = record; proposalTarget.value = null; layout.value?.inspect() }
function inspectProposal(target: ProposalTarget) {
  const p = target.proposal
  if (!execution.connected || !gateway.connected.value || p.run_id !== execution.runId || !p.binding?.scope || !p.binding.affected_nodes?.includes(target.nodeId)) return
  inspected.value = snapshot.value?.view?.invocations.findLast(i => i.node_id === target.nodeId && i.scope === p.binding?.scope) ?? null
  proposalTarget.value = target
  layout.value?.inspect()
}
function proposalVersion(target: ProposalVersion) {
  if (!execution.connected || !gateway.connected.value || target.proposal.run_id !== execution.runId) return
  versions.openReference(target.version, target.proposal.run_id!, target.label, target.proposal.proposal_id)
  openSurface('version')
}
watch(() => execution.current, () => { if (inspected.value) inspected.value = execution.current?.snapshot?.view?.invocations.find(i => i.sequence === inspected.value?.sequence) ?? inspected.value })
const { openSurface } = useSurfaceNavigation()
const versions = useVersionStore()
let timer: ReturnType<typeof setTimeout> | undefined, disposed = false
const now = ref(Date.now())
async function poll() { await execution.reconcile(); now.value = Date.now(); if (!disposed) timer = setTimeout(poll, 700) }
onMounted(() => { tabs.openSurface('execution'); panel.clear(); void poll() })
onBeforeUnmount(() => { disposed = true; clearTimeout(timer) })
watch(() => workspace.active?.path, () => { selected.value = null; path.value = ''; inspected.value = null; proposalTarget.value = null; error.value = ''; void execution.reconcile() })
watch(() => execution.runId, () => { inspected.value = null; proposalTarget.value = null })
watch(() => execution.connected && gateway.connected.value, connected => { if (!connected) proposalTarget.value = null })
const snapshot = computed(() => selected.value ? undefined : execution.current?.snapshot)
const runtime = computed(() => snapshot.value?.runtime)
const live = computed(() => execution.connected && gateway.connected.value && !!runtime.value?.active && !runtime.value.pause_requested && !runtime.value.cancel_requested && !runtime.value.pending_approval_ids.length)
const label = computed(() => selected.value ? 'Ready' : !execution.connected ? 'State unknown' : runtime.value?.cancel_requested ? 'Stopping' : runtime.value?.pause_requested ? 'Pause requested' : runtime.value?.pending_approval_ids.length ? 'Awaiting confirmation' : execution.status === 'idle' ? 'Ready' : execution.status)
const duration = computed(() => { const r = selected.value ? null : execution.current; return r ? `${Math.max(0, Math.round(((execution.active && execution.connected ? now.value : r.updatedAt) - r.startedAt) / 1000))}s` : '' })
function openVersion() { const v = inspected.value?.version ?? snapshot.value?.blueprint_version; if (v && execution.runId) { versions.openReference(v, execution.runId, 'Execution version'); openSurface('version') } }
async function pick(file: FilePick) {
  picker.value = false
  const ws = workspace.active?.path
  if (!ws || !file.filePath || execution.active) return
  busy.value = true; error.value = ''
  try {
    const read = await gateway.readFile(ws, file.filePath)
    if (!read.ok) throw new Error(read.error)
    const parsed = JSON.parse(read.data.content)
    if (!parsed.id) throw new Error('Save this blueprint in the editor before running.')
    const loaded = await gateway.loadBlueprint(ws, parsed.id)
    if (!loaded.ok) throw new Error(loaded.error)
    if (workspace.active?.path !== ws || execution.active) return
    selected.value = loaded.data; path.value = file.filePath; inspected.value = null
    execution.error = ''
  } catch (e) { error.value = String(e) } finally { busy.value = false }
}
function run() { const bp = graph.value, file = source.value; if (bp) { selected.value = null; void execution.run(bp.id, bp, file) } }
function openSource() { if (source.value) { tabs.openFile(source.value); void router.push(fileRoute(source.value)) } }
</script>
<template>
  <ExecutionLayout ref="layout">
    <template #header>
      <button class="blueprint-select" :disabled="execution.active || execution.launching || busy" @click="picker = true">{{ graph?.name || 'Select blueprint' }}</button>
      <span class="status">{{ label }}</span>
      <span class="status">{{ duration }}</span>
      <template v-if="execution.active"><button class="action" :disabled="execution.controlBusy || !execution.runId || !execution.connected" @click="runtime?.pause_requested ? execution.resume() : execution.pause()"><Play v-if="runtime?.pause_requested" :size="13" /><Pause v-else :size="13" />{{ runtime?.pause_requested ? 'Continue' : 'Pause' }}</button><button class="action stop" :disabled="execution.controlBusy || !execution.runId || !execution.connected" @click="execution.cancel()"><Square :size="13" />Stop</button></template>
      <button v-else-if="execution.current?.status === 'Suspended' && !selected" class="action" :disabled="execution.controlBusy || !execution.connected" @click="execution.recover()">Recover checkpoint</button>
      <button v-else class="action primary" :disabled="!graph || execution.active || execution.launching || busy" @click="run"><Play :size="13" />Run</button>
      <span v-if="error || execution.error" role="alert">{{ error || execution.error }}</span>
    </template>
    <template #tools><button aria-label="Open blueprint source" :disabled="!source" @click="openSource"><FileCode2 :size="15" /></button></template>
    <template #graph><ExecutionGraph v-if="graph" :key="execution.runId ?? graph.id" :blueprint="graph" :snapshot="snapshot" :live="live" @inspect="inspect" /><div v-else class="empty">{{ execution.runId ? 'Execution graph unavailable for this record.' : 'Select a saved blueprint to run.' }}</div></template>
    <template #supervision="{ tab }"><OversightReviews v-if="tab === 'Reviews'" :run-id="selected ? null : execution.runId" :connected="execution.connected && gateway.connected.value" @inspect="(id, scope) => inspect(snapshot?.view?.invocations.findLast(i => i.node_id === id && i.scope === scope) ?? null)" @target="inspectProposal" @version="proposalVersion" /><ConciergePanel v-else :run-id="selected ? null : execution.runId" :connected="execution.connected && gateway.connected.value" :tab="tab" @target="inspectProposal" @version="proposalVersion" /></template>
    <template #details="{ tab }">
      <div v-if="tab === 'Node details'" class="details node-details">
        <p v-if="execution.runId">Run: {{ execution.runId }}</p>
        <section v-if="proposalTarget" aria-label="Proposal target details"><strong>Proposal target · {{ proposalTarget.nodeId }}</strong><p>Proposal: {{ proposalTarget.proposal.proposal_id }} · Scope: {{ proposalTarget.proposal.binding?.scope }}</p><p v-if="!inspected">No recorded invocation for this target.</p><div class="io"><section><strong>Before proposal</strong><pre>{{ JSON.stringify(proposalTarget.proposal.binding?.before?.find(n => n.id === proposalTarget?.nodeId), null, 2) || 'Unavailable' }}</pre></section><section><strong>Proposed after</strong><pre>{{ JSON.stringify(proposalTarget.proposal.binding?.after?.find(n => n.id === proposalTarget?.nodeId), null, 2) || 'Unavailable' }}</pre></section></div></section>
        <button v-if="snapshot?.blueprint_version && !proposalTarget" @click="openVersion"><GitBranch :size="13" />Version Flow · {{ (inspected?.version ?? snapshot.blueprint_version).blob_hash.slice(0, 12) }}</button>
        <template v-if="inspected"><p>{{ inspected.node_id }} · Attempt {{ inspected.attempt }} · {{ inspected.status }}{{ inspected.current === false ? ' · Previous attempt' : '' }}</p><p>Frame: {{ inspected.frame.join(' / ') || 'Root' }}</p><div class="io"><section><strong>Inputs</strong><pre>{{ JSON.stringify(inspected.inputs, null, 2) }}</pre></section><section><strong>Outputs</strong><pre>{{ JSON.stringify(inspected.outputs, null, 2) }}</pre></section></div><p v-for="(message, i) in inspected.messages" :key="i">{{ message }}</p></template><p v-else-if="!proposalTarget">Select a node to inspect its execution.</p>
      </div>
      <div v-else-if="tab === 'Execution log'" class="details"><p v-if="!snapshot?.view?.root">Persisted execution details are unavailable.</p><p v-for="record in snapshot?.view?.invocations" :key="record.sequence"><button @click="inspect(record)">#{{ record.sequence }} · {{ record.node_id }} · {{ record.status }} · attempt {{ record.attempt }}</button><span v-for="(message, i) in record.messages" :key="i"> · {{ message }}</span></p><p v-for="(event, i) in execution.events" :key="i">{{ event.kind }} · {{ event.message }}</p></div>
      <ExecutionBlackboard v-else-if="tab === 'Blackboard'" :run-id="selected ? null : execution.runId" :connected="execution.connected && gateway.connected.value" :revision="execution.current?.updatedAt" />
      <ExecutionUsage v-else-if="tab === 'Usage'" :run-id="selected ? null : execution.runId" :connected="execution.connected && gateway.connected.value" />
      <div v-else class="details">{{ tab === 'Node details' ? 'Select a node to inspect its execution.' : `${tab} is not connected yet.` }}</div>
    </template>
  </ExecutionLayout>
  <FilePickerDialog :open="picker" mode="open" title="Select blueprint" :workspace-path="workspace.active?.path ?? ''" :extensions="['.blueprint']" @close="picker = false" @confirm="pick" />
</template>
<style scoped>
.blueprint-select{font-size:13px;max-width:300px;overflow:hidden;text-overflow:ellipsis}.status{color:var(--muted-foreground);font-size:12px}.action{display:flex;align-items:center;gap:6px;padding:7px 10px;border:1px solid var(--border);border-radius:6px;margin-left:auto}.action+.action{margin-left:0}.primary{background:var(--primary);color:white}button:disabled{opacity:.45}.io{display:grid;grid-template-columns:1fr 1fr;gap:24px}pre{white-space:pre-wrap}.node-details button{display:flex;align-items:center;gap:6px}.details{padding:20px;color:var(--muted-foreground);overflow-wrap:anywhere}.empty{height:100%;display:grid;place-items:center;color:var(--muted-foreground)}[role=alert]{color:var(--danger);width:100%}
</style>
