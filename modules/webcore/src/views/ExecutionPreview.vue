<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, provide, ref, watch } from 'vue'
import { VueFlow, useVueFlow, type Node, type Edge } from '@vue-flow/core'
import { Background } from '@vue-flow/background'
import { Controls } from '@vue-flow/controls'
import { Activity, ArrowUp, Check, Plus, ChevronRight, Clock3, Crosshair, Maximize2, PanelBottom, PanelRight, FileCode2, GitBranch, MessageSquare, Pause, Play, RotateCcw, ShieldCheck, Square, Workflow, X } from '@lucide/vue'
import '@vue-flow/core/dist/style.css'
import '@vue-flow/core/dist/theme-default.css'
import BlueprintNode from '@/components/BlueprintNode.vue'
import ChatThread from '@/components/chat/ChatThread.vue'
import ExecutionProposalPreview from '@/components/ExecutionProposalPreview.vue'
import type { ChatMessage } from '@/core'
import { usePanelStore } from '@/stores/panel'
import { useTabsStore } from '@/stores/tabs'

provide('bp-pin-context', () => {})

// Isolated visual prototype. No execution, approval or model RPCs are called.
type Scene = 'ready' | 'running' | 'paused' | 'approval' | 'failed' | 'finished' | 'cancelled'
const scene = ref<Scene>('running')
const selected = ref('implement')
const rightTab = ref('Chat')
const bottomTab = ref('Node details')
const showDetails = ref(true)
const showSupervisor = ref(true)
const previewOptions = ref(false)
const supervisorWidth = ref(420)
const resizing = ref(false)
const bodyEl = ref<HTMLElement | null>(null)
let resizeStartX = 0, resizeStartWidth = 420
function resizeTo(width: number) {
  const max = Math.max(280, Math.min(600, (bodyEl.value?.clientWidth ?? 1000) - 360))
  supervisorWidth.value = Math.round(Math.max(280, Math.min(max, width)))
}
function startResize(event: PointerEvent) {
  if (event.button !== 0) return
  event.preventDefault()
  resizing.value = true
  resizeStartX = event.clientX
  resizeStartWidth = supervisorWidth.value
  ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
}
function moveResize(event: PointerEvent) {
  if (resizing.value) resizeTo(resizeStartWidth + resizeStartX - event.clientX)
}
function endResize() {
  if (!resizing.value) return
  resizing.value = false
  void fit()
}
function keyboardResize(event: KeyboardEvent) {
  if (!['ArrowLeft', 'ArrowRight', 'Home'].includes(event.key)) return
  event.preventDefault()
  resizeTo(event.key === 'Home' ? 420 : supervisorWidth.value + (event.key === 'ArrowLeft' ? 20 : -20))
  void fit()
}
const defaultDetailsHeight = 200
const detailsHeight = ref(defaultDetailsHeight)
const resizingDetails = ref(false)
const mainColumn = ref<HTMLElement | null>(null)
const mainHeight = ref(720)
const maxDetailsHeight = computed(() => Math.max(120, Math.min(480, mainHeight.value - 280)))
let detailsStartY = 0, detailsStartHeight = defaultDetailsHeight
function resizeDetailsTo(height: number) {
  detailsHeight.value = Math.round(Math.max(120, Math.min(maxDetailsHeight.value, height)))
}
function startDetailsResize(event: PointerEvent) {
  if (event.button !== 0) return
  event.preventDefault()
  resizingDetails.value = true
  detailsStartY = event.clientY
  detailsStartHeight = detailsHeight.value
  ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
}
function moveDetailsResize(event: PointerEvent) {
  if (resizingDetails.value) resizeDetailsTo(detailsStartHeight + detailsStartY - event.clientY)
}
function endDetailsResize() {
  if (!resizingDetails.value) return
  resizingDetails.value = false
  void fit()
}
function keyboardDetailsResize(event: KeyboardEvent) {
  if (!['ArrowUp', 'ArrowDown', 'Home'].includes(event.key)) return
  event.preventDefault()
  resizeDetailsTo(event.key === 'Home' ? defaultDetailsHeight : detailsHeight.value + (event.key === 'ArrowUp' ? 20 : -20))
  void fit()
}
// Sample input includes cached input; output tokens are excluded from this ratio.
const sampleUsage = { inputTokens: 8000, cachedInputTokens: 5600 }
const cacheHitRate = computed(() => sampleUsage.inputTokens > 0 ? Math.round(sampleUsage.cachedInputTokens / sampleUsage.inputTokens * 100) : null)
const follow = ref(true)
const editorPreview = ref(false)
const proposal = ref<'pending' | 'approved' | 'applied' | 'denied'>('pending')
const chatThread = ref<InstanceType<typeof ChatThread> | null>(null)
function scrollConversation() { chatThread.value?.scrollToBottom(true) }
const draft = ref('')
const file = ref('fix-session-recovery.blueprint')
const elapsed = ref(154)
const step = ref(2)
const playing = ref(false)
const version = ref('a31c9f2')
const messages = ref([{ role: 'user', text: 'Fix session recovery, keep the API unchanged, and cover expired sessions.' },
  { role: 'assistant', text: 'The session restore races with the route guard. I am updating the initialization order, then checking refresh and sign-in behavior.' }])
const stages = [
  { id: 'start', name: 'Start', kind: 'Start', category: 'event', x: 0, y: 20, input: 'Task', output: 'Goal', summary: 'Read the task and its execution constraints.', time: '0.1s' },
  { id: 'inspect', name: 'Investigate', kind: 'Agent', category: 'module', x: 285, y: 20, input: 'Goal', output: 'Findings', summary: 'Confirmed a race between session restore and the route guard.', time: '42s' },
  { id: 'implement', name: 'Implement', kind: 'Agent', category: 'module', x: 570, y: 20, input: 'Findings', output: 'Changes', summary: 'Updating session restore order while preserving the API and permission checks.', time: '1m 51s' },
  { id: 'test', name: 'Validate', kind: 'Validator', category: 'action', x: 0, y: 185, input: 'Changes', output: 'Results', summary: 'Check refresh, expired sessions, and sign-in paths.', time: 'Pending' },
  { id: 'review', name: 'Review', kind: 'Judge', category: 'flow', x: 285, y: 185, input: 'Results', output: 'Verdict', summary: 'Check the result against the task and report unresolved issues.', time: 'Pending' },
  { id: 'end', name: 'Deliver', kind: 'End', category: 'event', x: 570, y: 185, input: 'Verdict', output: 'Report', summary: 'Summarize changes, validation evidence, and unfinished work.', time: 'Pending' },
]
const sceneLabels: Record<Scene, string> = { ready: 'Ready', running: 'Running', paused: 'Paused', approval: 'Awaiting confirmation', failed: 'Failed', finished: 'Completed', cancelled: 'Stopped' }
const status = computed(() => sceneLabels[scene.value])
const clock = computed(() => `${Math.floor(elapsed.value / 60).toString().padStart(2, '0')}:${(elapsed.value % 60).toString().padStart(2, '0')}`)
const inspected = computed(() => stages.find(n => n.id === selected.value) ?? stages[2]!)
function nodeState(index: number) {
  if (scene.value === 'ready') return 'Pending'
  if (scene.value === 'finished' || index < step.value) return 'Completed'
  if (index > step.value) return 'Pending'
  return ({ running: 'Running', paused: 'Paused', approval: 'Awaiting confirmation', failed: 'Failed', cancelled: 'Stopped' } as Record<string, string>)[scene.value] ?? 'Pending'
}
const nodes = computed<Node[]>(() => stages.map((n, i) => ({
  id: n.id, type: 'preview', position: { x: n.x, y: n.y }, selectable: true,
  data: { title: n.name, category: n.category, kind: n.kind, state: nodeState(i),
    status: nodeState(i) === 'Running' ? 'running' : nodeState(i) === 'Completed' ? 'done' : undefined,
    inputs: [ ...(i ? [{ id: `${n.id}-in`, name: 'Exec', kind: 'exec-in', type: 'exec' }] : []),
      { id: `${n.id}-data-in`, name: n.input, kind: 'data-in', type: 'string' } ],
    outputs: [ ...(i < 5 ? [{ id: `${n.id}-out`, name: 'Continue', kind: 'exec-out', type: 'exec' }] : []),
      { id: `${n.id}-data-out`, name: n.output, kind: 'data-out', type: 'string' } ],
    connectedIn: [`${n.id}-in`, `${n.id}-data-in`], connectedOut: [`${n.id}-out`, `${n.id}-data-out`],
  },
})))
const edges = computed<Edge[]>(() => stages.slice(0, -1).flatMap((n, i) => {
  const next = stages[i + 1]!
  const passed = scene.value !== 'ready' && (i < step.value || scene.value === 'finished')
  const active = i === step.value - 1 && scene.value === 'running'
  return [{ id: `exec-${i}`, source: n.id, target: next.id, sourceHandle: `${n.id}-out`, targetHandle: `${next.id}-in`,
    type: 'default', animated: active, class: `metteur-edge--exec ${passed ? 'is-traversed' : ''}`,
    style: { stroke: active ? 'var(--primary)' : passed ? '#2fbf8f' : 'var(--subtle)', strokeWidth: active ? 3 : 1.5, opacity: passed ? 1 : 0.38 } },
  { id: `data-${i}`, source: n.id, target: next.id, sourceHandle: `${n.id}-data-out`, targetHandle: `${next.id}-data-in`,
    type: 'default', class: 'metteur-edge--data', style: { stroke: '#ec6b7e', strokeWidth: 1, opacity: 0.23 } }]
}))
const { fitView } = useVueFlow('execution-preview')
async function fit() { await nextTick(); await fitView({ padding: 0.22, duration: 250 }) }
watch([showDetails, showSupervisor], () => { void fit() })
watch(() => messages.value.length, () => { void scrollConversation() })
function setScene(value: Scene) {
  scene.value = value
  version.value = 'a31c9f2'
  playing.value = false
  step.value = value === 'finished' ? 6 : value === 'failed' ? 3 : value === 'ready' ? 0 : 2
  selected.value = stages[Math.min(step.value, 5)]!.id
  proposal.value = 'pending'
}
function advance() {
  if (scene.value !== 'running') return
  step.value++
  if (step.value >= stages.length) { scene.value = 'finished'; playing.value = false }
  if (proposal.value === 'approved') { proposal.value = 'applied'; version.value = 'b74e210' }
  if (follow.value) selected.value = stages[Math.min(step.value, 5)]!.id
}
function approve() {
  proposal.value = 'approved'
  scene.value = 'running'
  messages.value.push({ role: 'system', text: 'Change approved. Waiting for a safe boundary; the active blueprint version has not changed.' })
}
const attachedNode = ref<string | null>(null)
const availability = ref<'available' | 'unconfigured' | 'budget'>('available')
const requests = ref<{ id: string; text: string; state: 'received' | 'withdrawn'; node: string | null }[]>([])
const requestFilter = ref<'all' | 'open'>('all')
const copied = ref(false)
const runClosed = computed(() => ['finished', 'cancelled'].includes(scene.value))
const canSend = computed(() => scene.value !== 'ready' && !runClosed.value && availability.value === 'available')
const requestStatus = computed(() => runClosed.value && ['pending', 'approved'].includes(proposal.value) ? 'Closed without applying' : proposal.value === 'pending' ? (runClosed.value ? 'Closed without applying' : 'Awaiting confirmation') : proposal.value === 'approved' ? 'Awaiting safe boundary' : proposal.value === 'applied' ? 'Applied' : 'Rejected')
const pendingCount = computed(() => runClosed.value ? 0 : requests.value.filter(r => r.state === 'received').length + (['pending', 'approved'].includes(proposal.value) && !runClosed.value ? 1 : 0))
const transcript = computed<ChatMessage[]>(() => messages.value.map((m, i) => ({ id: `preview-${i}`, role: m.role === 'system' ? 'notice' : m.role as 'user' | 'assistant', content: m.text, createdAt: new Date('2026-10-04T10:24:00').getTime() + i * 60000 })))
const inputHint = computed(() => runClosed.value ? 'This run is closed. The conversation is read-only.' : scene.value === 'ready' ? 'Start a run to open the conversation.' : availability.value === 'unconfigured' ? 'No concierge model configured. Run controls remain available.' : availability.value === 'budget' ? 'Oversight budget reached. Existing requests are preserved.' : 'Preview only · No model calls')
async function copyMessage(text: string) {
  try { await navigator.clipboard.writeText(text); copied.value = true } catch { copied.value = false }
}
function editMessage(text: string) { if (canSend.value) { draft.value = text; rightTab.value = 'Chat' } }
function retryMessage() { if (canSend.value) send('What is the current progress?') }
function inspectValidation() { selected.value = 'test'; showDetails.value = true }
function reject() { proposal.value = 'denied'; if (scene.value === 'approval') scene.value = 'running' }
function send(text = draft.value) {
  if (!text.trim() || !canSend.value) return
  rightTab.value = 'Chat'
  copied.value = false
  const content = text.trim()
  const node = attachedNode.value
  messages.value.push({ role: 'user', text: content + (node ? `\n\nNode: **${node}**` : '') })
  draft.value = ''
  attachedNode.value = null
  const query = /progress|status|where/i.test(content)
  if (query) {
    messages.value.push({ role: 'assistant', text: `**${status.value}** · ${Math.min(step.value, 6)} of 6 nodes completed.\n\nCurrent node: **${stages[Math.min(step.value, 5)]!.name}**. The active blueprint version is \`${version.value}\`. This is a simulated response.` })
  } else {
    const id = `R-${String(requests.value.length + 2).padStart(3, '0')}`
    requests.value.push({ id, text: content, state: 'received', node })
    messages.value.push({ role: 'assistant', text: `Recorded as **${id}** in the preview request queue. It is awaiting review; no change has been made.\n\nYou can inspect or withdraw it in **Requests**. Any proposed action will need a separate confirmation.` })
  }
}
const graphArea = ref<HTMLElement | null>(null)
let graphObserver: ResizeObserver | undefined
let columnObserver: ResizeObserver | undefined
let fitFrame = 0
let timer: ReturnType<typeof setInterval> | undefined
let ticks = 0
onMounted(async () => {
  usePanelStore().clear()
  useTabsStore().openSurface('execution-preview')
  graphObserver = new ResizeObserver(() => {
    cancelAnimationFrame(fitFrame)
    fitFrame = requestAnimationFrame(() => {
      fitFrame = requestAnimationFrame(() => { if (!resizing.value && !resizingDetails.value) void fit() })
    })
  })
  if (graphArea.value) graphObserver.observe(graphArea.value)
  columnObserver = new ResizeObserver(() => {
    mainHeight.value = mainColumn.value?.clientHeight ?? 720
    resizeDetailsTo(detailsHeight.value)
  })
  if (mainColumn.value) columnObserver.observe(mainColumn.value)
  void scrollConversation()
  timer = setInterval(() => {
    if (scene.value === 'running' && playing.value) {
      elapsed.value++
      if (++ticks % 5 === 0) advance()
    }
  }, 1000)
})
onBeforeUnmount(() => { clearInterval(timer); graphObserver?.disconnect(); columnObserver?.disconnect(); cancelAnimationFrame(fitFrame) })
</script>

<template>
  <div class="execution-preview">
    <header class="run-header">
      <div class="run-heading">
        <Workflow :size="15" class="muted" />
        <select v-model="file" class="blueprint-select" aria-label="Select blueprint" :disabled="!['ready', 'finished', 'failed', 'cancelled'].includes(scene)" @change="setScene('ready')"><option>fix-session-recovery.blueprint</option><option>add-search.blueprint</option></select>
        <span class="status-pill" :class="scene">{{ status }}</span>
      </div>
      <div class="run-actions">
        <span class="run-clock"><Clock3 :size="12" />{{ clock }}</span>
        <div class="preview-options">
          <button class="preview-toggle" :aria-expanded="previewOptions" @click="previewOptions = !previewOptions">Demo</button>
          <div v-if="previewOptions" class="preview-popover">
            <strong>Interactive preview</strong><p>Sample data. No tasks or model calls.</p>
            <label>Scene<select :value="scene" aria-label="Preview scene" @change="setScene(($event.target as HTMLSelectElement).value as Scene)"><option v-for="(label, key) in sceneLabels" :key="key" :value="key">{{ label }}</option></select></label>
            <label>Concierge<select v-model="availability" aria-label="Concierge availability"><option value="available">Available (simulated)</option><option value="unconfigured">Not configured</option><option value="budget">Budget reached</option></select></label>
            <button class="action" @click="setScene('running'); elapsed = 154; previewOptions = false"><RotateCcw :size="12" />Reset preview</button>
          </div>
        </div>
        <template v-if="['ready', 'finished', 'failed', 'cancelled'].includes(scene)"><button class="action primary" @click="setScene('running'); playing = true"><Play :size="13" />{{ scene === 'ready' ? 'Run' : 'Run again' }}</button></template>
        <template v-else><button class="action" @click="scene = scene === 'paused' ? 'running' : 'paused'"><Play v-if="scene === 'paused'" :size="13" /><Pause v-else :size="13" />{{ scene === 'paused' ? 'Continue' : 'Pause' }}</button><button class="action stop" @click="scene = 'cancelled'; playing = false"><Square :size="12" />Stop</button></template>
      </div>
    </header>

    <div ref="bodyEl" class="run-body" :class="{ resizing, 'resizing-details': resizingDetails }">
      <section ref="mainColumn" class="main-column">
        <div ref="graphArea" class="graph-area">
          <div class="floating-toolbar" role="toolbar" aria-label="Blueprint view tools">
            <button :class="{ active: follow }" :aria-pressed="follow" aria-label="Follow execution" title="Follow execution" @click="follow = !follow"><Crosshair :size="15" /></button>
            <button aria-label="Fit graph" title="Fit graph" @click="fit"><Maximize2 :size="15" /></button>
            <span class="tool-divider" />
            <button :class="{ active: showDetails }" :aria-pressed="showDetails" aria-label="Node details panel" title="Node details panel" @click="showDetails = !showDetails"><PanelBottom :size="15" /></button>
            <button :class="{ active: showSupervisor }" :aria-pressed="showSupervisor" aria-label="Supervision panel" title="Supervision panel" @click="showSupervisor = !showSupervisor"><PanelRight :size="15" /></button>
            <button aria-label="Open blueprint source" title="Open blueprint source" @click="editorPreview = true"><FileCode2 :size="15" /></button>
          </div>
          <VueFlow id="execution-preview" class="blueprint-flow execution-flow" :nodes="nodes" :edges="edges" :nodes-draggable="false" :nodes-connectable="false" :zoom-on-double-click="false" :min-zoom="0.35" :max-zoom="1.5" :fit-view-on-init="true" @nodes-initialized="fit" @node-click="({ node }) => { selected = node.id; showDetails = true }">
            <Background :gap="20" :size="1" pattern-color="var(--grid-dot)" />
            <Controls position="bottom-left" :show-interactive="false" />
            <template #node-preview="{ id, data }"><div class="runtime-node" :class="[{ inspected: selected === id }, data.state === 'Failed' ? 'node-failed' : '', data.state === 'Awaiting confirmation' ? 'node-waiting' : '']">
              <BlueprintNode :id="id" :data="data" :selected="selected === id" />
              <div class="node-caption"><span>{{ data.kind }}</span><span :class="{ success: data.state === 'Completed', accent: data.state === 'Running', danger: data.state === 'Failed' }"><Check v-if="data.state === 'Completed'" :size="10" /><span v-if="data.state === 'Running'" class="live-dot" />{{ data.state }}</span></div>
            </div></template>
          </VueFlow>
          <div v-if="scene === 'failed'" class="graph-notice error"><Activity :size="14" /><span>Validation failed: expired-session test did not pass.</span><button @click="selected = 'test'; showDetails = true">Inspect node</button></div>
          <div v-else-if="scene === 'paused' || scene === 'approval'" class="graph-notice"><Pause :size="13" /><span>{{ scene === 'paused' ? 'Paused. Current progress is preserved.' : 'Waiting for confirmation. No change has been applied.' }}</span></div>
          <div v-else-if="scene === 'finished'" class="graph-notice success"><Check :size="14" /><span>Run completed. Results and evidence are available below.</span></div>

          <div class="demo-playback"><button :disabled="scene !== 'running'" @click="playing = !playing"><Pause v-if="playing" :size="11" /><Play v-else :size="11" />{{ playing ? 'Pause demo' : 'Play demo' }}</button><button :disabled="scene !== 'running'" @click="advance">Next node <ChevronRight :size="11" /></button></div>
        </div>

        <section v-if="showDetails" class="details-panel" :style="{ height: detailsHeight + 'px' }">
          <div class="details-resizer" role="separator" aria-label="Resize details panel" aria-orientation="horizontal" :aria-valuenow="detailsHeight" :aria-valuemin="120" :aria-valuemax="maxDetailsHeight" tabindex="0" title="Drag to resize; double-click to reset" @pointerdown="startDetailsResize" @pointermove="moveDetailsResize" @pointerup="endDetailsResize" @pointercancel="endDetailsResize" @lostpointercapture="endDetailsResize" @keydown="keyboardDetailsResize" @dblclick="resizeDetailsTo(defaultDetailsHeight); fit()" />
          <div class="panel-tabs"><button v-for="tab in ['Node details', 'Blackboard', 'Execution log', 'Usage']" :key="tab" :class="{ active: bottomTab === tab }" @click="bottomTab = tab">{{ tab }}<span v-if="tab === 'Blackboard'" class="count">3</span></button><button class="close-detail" aria-label="Close details" @click="showDetails = false"><X :size="13" /></button></div>
          <div class="details-content">
          <div v-if="bottomTab === 'Node details'" class="node-details">
            <div class="detail-summary"><div><strong>{{ inspected.name }}</strong><span class="small-badge">{{ nodeState(stages.indexOf(inspected)) }}</span></div><p>{{ inspected.summary }}</p><div class="detail-facts"><span>Attempt <b>01</b></span><span>Duration <b>{{ inspected.time }}</b></span><span>Node <b class="mono">{{ inspected.id }}</b></span></div></div>
            <div class="evidence"><span class="eyebrow">{{ scene === 'failed' && selected === 'test' ? 'Error evidence' : 'Latest activity · Sample' }}</span><p class="mono">{{ scene === 'failed' && selected === 'test' ? 'FAIL session.spec.ts › expired session redirects' : selected === 'implement' ? 'src/stores/session.ts  +18 −6' : 'Node inputs and results appear here.' }}</p><span class="muted">{{ selected === 'implement' ? 'The result summary is published when this node finishes.' : 'Execution facts and model opinions remain separate.' }}</span></div>
          </div>
          <div v-else-if="bottomTab === 'Blackboard'" class="board-rows"><div><Check :size="13" class="success" /><strong>Investigation complete</strong><span>Execution fact</span><span class="muted">Session initialization has a race condition.</span></div><div><ShieldCheck :size="13" /><strong>Keep the API unchanged</strong><span>User constraint</span><span class="muted">Applies throughout this run.</span></div><div><MessageSquare :size="13" /><strong>Add expired-session coverage</strong><span>Model suggestion</span><span class="muted">Not applied. Confirmation required.</span></div></div>
          <div v-else-if="bottomTab === 'Execution log'" class="log-rows mono"><p><span>10:24:00</span> run.started <b>Blueprint version locked to a31c9f2</b></p><p><span>10:24:01</span> node.finished <b>Start → Investigate</b></p><p><span>10:24:43</span> node.started <b>Implement · attempt 1</b></p><p><span>10:26:34</span> {{ scene === 'failed' ? 'node.failed' : 'run.observed' }} <b>{{ status }} · Demo event</b></p></div>
          <div v-else class="usage-grid">
            <div><span>Execution · Sample</span><strong>8,420 <small>tokens</small></strong></div>
            <div><span>Oversight · Sample</span><strong>1,180 <small>tokens</small></strong></div>
            <div class="cache-metric"><span>Cache hit rate · Sample</span><strong>{{ cacheHitRate ?? 'Unavailable' }}<small v-if="cacheHitRate !== null">%</small></strong><meter v-if="cacheHitRate !== null" :value="cacheHitRate" min="0" max="100" aria-label="Sample input token cache hit rate" /></div>
            <div><span>Oversight budget · Sample</span><strong>1.8<small>%</small></strong></div>
            <div><span>Actual cost</span><strong class="muted">Unavailable</strong></div>
          </div>
          </div>
        </section>
      </section>

      <aside v-if="showSupervisor" class="supervisor chat-surface" :style="{ width: supervisorWidth + 'px' }">
        <div class="supervisor-resizer" role="separator" aria-label="Resize supervision panel" aria-orientation="vertical" :aria-valuenow="supervisorWidth" :aria-valuemin="280" :aria-valuemax="600" tabindex="0" title="Drag to resize; double-click to reset" @pointerdown="startResize" @pointermove="moveResize" @pointerup="endResize" @lostpointercapture="endResize" @keydown="keyboardResize" @dblclick="resizeTo(420); fit()" />
        <div class="supervisor-heading"><ShieldCheck :size="15" class="muted" /><strong>Supervision</strong><span class="small-badge">Assisted</span></div>
        <div class="panel-tabs"><button v-for="tab in ['Chat', 'Requests', 'Reviews']" :key="tab" :class="{ active: rightTab === tab }" @click="rightTab = tab">{{ tab }}<span v-if="tab === 'Requests' && pendingCount" class="count">{{ pendingCount }}</span></button></div>
        <button v-if="proposal === 'pending' && !runClosed && scene !== 'ready'" class="pending-review" @click="rightTab = 'Requests'; requestFilter = 'all'"><ShieldCheck :size="14" /><span>1 change needs your review</span><ChevronRight :size="14" /></button>
        <ChatThread v-if="rightTab === 'Chat'" ref="chatThread" :messages="scene === 'ready' ? [] : transcript" :on-open-file="() => { editorPreview = true }" :on-copy="copyMessage" :on-edit="editMessage" :on-retry="retryMessage" :on-restore="() => {}">
          <template #empty><div class="empty-message"><MessageSquare :size="24" /><h3>Stay in the loop</h3><p>Start a blueprint to ask about progress, clarify a requirement, or review a proposed change.</p></div></template>
          <template #footer>
            <ExecutionProposalPreview v-if="scene !== 'ready'" :state="proposal" :readonly="runClosed || scene === 'ready'" @approve="approve" @reject="reject" @inspect="inspectValidation" />
            <p v-if="runClosed" class="chat-notice">Run {{ scene === 'finished' ? 'completed' : 'stopped' }}. Conversation and requests are read-only.</p>
          </template>
        </ChatThread>
        <div v-else class="supervision-records">
          <template v-if="rightTab === 'Requests'">
            <div class="record-filter"><span>{{ requests.length + 1 }} requests</span><div><button :class="{ active: requestFilter === 'all' }" @click="requestFilter = 'all'">All</button><button :class="{ active: requestFilter === 'open' }" @click="requestFilter = 'open'">Open</button></div></div>
            <article v-if="requestFilter === 'all' || (!runClosed && ['pending', 'approved'].includes(proposal))" class="request-record">
              <div class="record-meta"><span>R-001 · User request</span><span>{{ requestStatus }}</span></div>
              <h3>Add expired-session coverage</h3><p>Keep the existing API and verify redirects for expired credentials.</p>
              <ol class="request-timeline"><li class="complete"><Check :size="12" />Received<span>10:24</span></li><li class="complete"><Check :size="12" />Reviewed<span>10:26</span></li><li :class="{ complete: proposal !== 'pending' }"><span class="timeline-dot" />{{ requestStatus }}</li></ol>
              <ExecutionProposalPreview :state="proposal" :readonly="runClosed || scene === 'ready'" @approve="approve" @reject="reject" @inspect="inspectValidation" />
            </article>
            <article v-for="request in requests.filter(r => requestFilter === 'all' || (!runClosed && r.state === 'received'))" :key="request.id" class="request-record">
              <div class="record-meta"><span>{{ request.id }} · User request</span><span>{{ request.state === 'withdrawn' ? 'Withdrawn' : runClosed ? 'Unprocessed' : 'Awaiting review' }}</span></div><p>{{ request.text }}</p><p v-if="request.node" class="context-reference">Node: {{ request.node }}</p><button v-if="request.state === 'received' && !runClosed" class="record-action" @click="request.state = 'withdrawn'">Withdraw request</button>
            </article>
            <p v-if="requestFilter === 'open' && !pendingCount" class="empty-message">No open requests.</p>
          </template>
          <template v-else>
            <article class="review-record"><div class="record-meta"><span>REVIEW 001</span><span>10:26 · Simulated</span></div><h3>One coverage gap found</h3><p>The implementation preserves the API. Add an expired-session check before accepting the result.</p>
              <div class="review-section"><h4>Evidence</h4><button @click="selected = 'inspect'; showDetails = true"><Check :size="14" />Investigation completed<ChevronRight :size="12" /></button><button @click="inspectValidation"><GitBranch :size="14" />Validation coverage<ChevronRight :size="12" /></button></div>
              <div class="review-section"><h4>Scope</h4><p>Change one parameter on Validate. Keep completed nodes and tool permissions unchanged.</p></div>
              <div class="review-section"><h4>Source</h4><p>User request R-001. Confirmation is required.</p></div>
              <ExecutionProposalPreview :state="proposal" :readonly="runClosed || scene === 'ready'" @approve="approve" @reject="reject" @inspect="inspectValidation" />
            </article>
          </template>
        </div>
        <div class="supervision-composer chat-composer-dock">
          <div v-if="availability !== 'available' || runClosed || scene === 'ready'" class="service-notice">{{ inputHint }}</div>
          <div v-else class="quick-prompts"><button @click="send('What is the progress?')">Check progress</button><button @click="draft = 'Please also verify '">Add a requirement</button></div>
          <form class="chat-composer" @submit.prevent="send()">
            <div v-if="attachedNode" class="node-attachment"><GitBranch :size="13" />{{ attachedNode }}<button type="button" aria-label="Remove node context" @click="attachedNode = null"><X :size="12" /></button></div>
            <textarea v-model="draft" class="chat-composer-textarea" aria-label="Supervision message" :disabled="!canSend" placeholder="Ask or request a change…" rows="2" @keydown.enter.exact.prevent="send()" />
            <div class="chat-composer-controls"><button type="button" class="chat-action attach-node" :disabled="!canSend" title="Attach the selected node" aria-label="Attach selected node" @click="attachedNode = inspected.name"><Plus :size="16" /></button><span class="composer-model">Concierge <span class="muted">· Preview</span></span><button class="chat-send" type="submit" aria-label="Send message" :disabled="!canSend || !draft.trim()"><ArrowUp :size="17" /></button></div>
          </form>
          <div class="composer-footnote"><span>{{ copied ? 'Copied to clipboard' : 'No model calls · Actions require confirmation' }}</span><span>Enter ↵</span></div>
        </div>
      </aside>
    </div>
    <div v-if="editorPreview" class="preview-modal-backdrop" @click.self="editorPreview = false"><section class="preview-modal"><header><FileCode2 :size="17" /><strong>{{ file }}</strong><button class="action" @click="editorPreview = false"><X :size="14" />Close</button></header><p>The final version will open this file in the existing editor and keep the execution tab. This preview shows a version summary.</p><div class="source-summary mono">blueprints/{{ file }}<br /><br />Active version: {{ version }}<br />6 nodes · 5 execution edges<br /><br />Start → Agent → Agent → Validator → Judge → End</div><p class="muted">The runtime graph is read-only. Editor drafts do not replace the active plan.</p></section></div>
  </div>
</template>

<style scoped>
.execution-preview { height:100%; display:flex; flex-direction:column; color:var(--foreground); background:var(--surface); font-size:12px }
button, select { cursor:pointer } button:disabled, select:disabled { cursor:default; opacity:.45 } button { transition:background .15s } button:hover:not(:disabled) { background:var(--hover) } button:focus-visible, select:focus-visible, textarea:focus-visible { outline:2px solid var(--ring); outline-offset:2px }
.muted { color:var(--muted-foreground) }.mono { font-family:ui-monospace,SFMono-Regular,Consolas,monospace }.success { color:#229d77 }.accent { color:var(--primary) }.danger { color:var(--danger) }

.eyebrow { display:flex; gap:5px; align-items:center; font-size:10px; color:var(--muted-foreground); letter-spacing:.04em }.status-pill { font-size:10px; letter-spacing:0; border-radius:5px; padding:3px 8px; color:var(--primary); background:var(--primary-soft); font-weight:500 }.status-pill.failed,.status-pill.cancelled { color:var(--danger); background:var(--danger-soft) }.status-pill.paused,.status-pill.approval { color:#b37c22; background:#b37c2215 }.status-pill.finished { color:#229d77; background:#229d7715 }
.run-actions { display:flex; gap:7px }.action { display:inline-flex; align-items:center; justify-content:center; gap:6px; padding:7px 10px; border:1px solid var(--border); border-radius:6px; background:var(--surface); font-size:11px; white-space:nowrap }.action.primary { background:var(--primary); border-color:var(--primary); color:white }.action.primary:hover { filter:brightness(1.08) }.action.stop { color:var(--danger) }

.run-body { display:flex; flex:1; min-height:0 }.main-column { flex:1; min-width:0; display:flex; flex-direction:column }
.graph-area { position:relative; flex:1; min-height:280px; background:var(--background) }.runtime-node { width:210px }.runtime-node :deep(.metteur-node) { width:210px; min-width:210px }.runtime-node :deep(input),.runtime-node :deep(select) { pointer-events:none }.node-caption { display:flex; justify-content:space-between; padding:8px 3px; color:var(--subtle); font-size:10px }.node-caption>span { display:flex; gap:4px; align-items:center }.node-caption .live-dot { margin:0 }.node-failed :deep(.metteur-node) { box-shadow:0 0 0 2px var(--danger) }.node-waiting :deep(.metteur-node) { box-shadow:0 0 0 2px #b37c22 }.demo-playback { position:absolute; bottom:14px; right:14px; display:flex; padding:3px; background:var(--surface); border:1px solid var(--border); border-radius:6px; box-shadow:0 2px 8px #00000005 }.demo-playback button { display:flex; align-items:center; gap:5px; padding:5px 8px; font-size:10px; color:var(--muted-foreground) }.graph-notice { position:absolute; z-index:5; left:50%; top:42px; transform:translateX(-50%); display:flex; align-items:center; gap:8px; padding:9px 12px; background:var(--surface); border:1px solid var(--border); border-radius:7px; box-shadow:var(--shadow-card); white-space:nowrap; font-size:11px }.graph-notice.error { color:var(--danger) }.graph-notice button { text-decoration:underline }
.details-panel { position:relative; display:flex; flex-direction:column; flex-shrink:0; border-top:1px solid var(--border); background:var(--surface) }.panel-tabs { flex-shrink:0; display:flex; align-items:center; gap:19px; padding:0 17px; height:38px; border-bottom:1px solid var(--divider) }.panel-tabs>button { height:100%; display:flex; align-items:center; gap:5px; position:relative; color:var(--muted-foreground); font-size:11px }.panel-tabs>button.active { color:var(--foreground); font-weight:600 }.panel-tabs>button.active:after { position:absolute; content:''; height:2px; background:var(--primary); bottom:0; left:0; right:0; border-radius:2px }.count { font-size:9px; background:var(--secondary); color:var(--muted-foreground); border-radius:4px; padding:0 4px }.close-detail { margin-left:auto }.node-details { display:grid; grid-template-columns:1fr 1fr; gap:25px; padding:15px 20px; font-size:11px }.node-details strong { font-size:12px }.small-badge { font-size:9px; padding:2px 6px; border:1px solid var(--border); border-radius:4px; color:var(--muted-foreground); font-weight:400; margin-left:9px }.node-details p { color:var(--muted-foreground); line-height:1.8; margin:9px 0 }.detail-facts { display:flex; gap:18px; color:var(--subtle); font-size:10px }.detail-facts b { color:var(--muted-foreground); margin-left:6px; font-weight:400 }.evidence { border-left:1px solid var(--divider); padding-left:24px }.evidence .mono { color:var(--foreground) }.evidence>span:last-child { font-size:10px }.board-rows,.log-rows { padding:11px 20px; font-size:10px }.board-rows>div { display:flex; align-items:center; gap:10px; margin:8px 0 }.board-rows strong { width:130px; font-weight:500 }.board-rows>div>span:not(.muted) { font-size:9px; background:var(--secondary); padding:1px 5px; border-radius:4px }.log-rows p { display:flex; gap:20px; padding:4px 0; color:var(--muted-foreground) }.log-rows b { font-weight:400; color:var(--foreground) }.usage-grid { padding:20px 22px; display:grid; grid-template-columns:repeat(auto-fit,minmax(140px,1fr)); gap:22px 18px }.usage-grid span { color:var(--muted-foreground); font-size:10px }.usage-grid strong { display:block; font-size:23px; font-weight:500; margin-top:8px }.usage-grid small { font-size:11px; color:var(--subtle) }
.supervisor { position:relative; width:350px; flex-shrink:0; display:flex; flex-direction:column; border-left:1px solid var(--border); background:var(--surface) }.supervisor>.panel-tabs { gap:24px }
.quick-prompts { display:flex; gap:6px; margin-bottom:9px }.quick-prompts button { font-size:9px; padding:4px 7px; border:1px solid var(--border); border-radius:5px; color:var(--muted-foreground) }.end-message,.empty-message { padding:22px 6px; color:var(--muted-foreground); line-height:1.9; font-size:11px }.empty-message { text-align:center }.empty-message svg { margin:0 auto 12px }
.preview-modal-backdrop { position:fixed; inset:0; z-index:100; background:#0005; display:grid; place-items:center; backdrop-filter:blur(3px) }.preview-modal { width:min(540px,90vw); padding:22px; background:var(--surface); border:1px solid var(--border); border-radius:12px; box-shadow:0 20px 70px #0003 }.preview-modal header { display:flex; align-items:center; gap:9px }.preview-modal header button { margin-left:auto }.preview-modal p { margin:18px 0; line-height:1.8; font-size:12px }.source-summary { padding:18px; background:var(--surface-muted); border-radius:7px; font-size:11px; line-height:1.8 }
@media(min-width:1550px) {  }
@media(max-width:1050px) { .run-actions>.source-link { display:none } }
@media(max-width:760px) { .run-body { flex-direction:column; overflow:auto }.main-column { min-height:570px; flex-shrink:0 }.supervisor { width:100%!important; min-height:540px; border-top:1px solid var(--border); border-left:0 }.node-details { padding:12px; gap:10px } }
@media(prefers-reduced-motion:reduce) { :deep(.vue-flow__edge-path) { animation:none!important } :deep(.metteur-node__live-dot) { animation:none!important } }

.live-dot { display:inline-block; width:6px; height:6px; border-radius:50%; background:var(--primary); margin-right:5px }
.run-header { display:flex; justify-content:space-between; align-items:center; gap:12px; height:49px; padding:0 17px; flex-shrink:0; border-bottom:1px solid var(--divider) }
.run-heading { display:flex; align-items:center; gap:10px; min-width:0 }
.blueprint-select { color:var(--foreground); background:transparent; font-size:13px; font-weight:500; max-width:270px; min-width:0 }
.blueprint-select:disabled { opacity:1 }
.run-clock { display:flex; align-items:center; gap:5px; font-size:10px; color:var(--muted-foreground); margin-right:6px }
.run-actions { align-items:center }
.preview-options { position:relative }
.preview-toggle { font-size:10px; color:var(--muted-foreground); padding:5px 8px; margin-right:5px; border:1px dashed var(--border); border-radius:5px }
.preview-popover { position:absolute; top:35px; right:0; width:245px; padding:15px; border:1px solid var(--border); border-radius:9px; background:var(--popover); box-shadow:0 8px 28px #0002; z-index:30; font-size:11px }
.preview-popover p { color:var(--muted-foreground); font-size:10px; margin:7px 0 14px }
.preview-popover label { display:flex; align-items:center; justify-content:space-between; gap:10px; margin-bottom:14px }
.preview-popover select { background:var(--input); border:1px solid var(--border); border-radius:5px; padding:4px }
.floating-toolbar { position:absolute; top:12px; right:13px; z-index:6; display:flex; align-items:center; gap:3px; padding:4px; background:var(--surface-glass); backdrop-filter:blur(12px); border:1px solid var(--border); border-radius:8px; box-shadow:0 2px 9px #0000000a }
.floating-toolbar button { display:grid; place-items:center; width:29px; height:28px; color:var(--muted-foreground); border-radius:5px }
.floating-toolbar button.active { color:var(--primary); background:var(--primary-soft) }
.tool-divider { width:1px; height:15px; background:var(--border); margin:0 3px }
.supervisor-heading { display:flex; align-items:center; gap:7px; height:44px; padding:0 17px; flex-shrink:0 }
.supervisor-heading strong { font-size:14px; font-weight:600 }
.supervisor-heading .small-badge { margin-left:auto }
.supervisor-resizer { position:absolute; left:-4px; top:0; bottom:0; width:8px; z-index:12; cursor:col-resize; touch-action:none }
.supervisor-resizer:after { content:''; position:absolute; left:3px; top:0; bottom:0; width:2px; transition:background .15s }
.supervisor-resizer:hover:after,.supervisor-resizer:focus-visible:after,.resizing .supervisor-resizer:after { background:var(--primary) }
.resizing { cursor:col-resize; user-select:none }
.details-content { flex:1; min-height:0; overflow:auto }
.details-resizer { position:absolute; top:-4px; left:0; right:0; height:8px; z-index:12; cursor:row-resize; touch-action:none }
.details-resizer:after { content:''; position:absolute; top:3px; left:0; right:0; height:2px; transition:background .15s }
.details-resizer:hover:after,.details-resizer:focus-visible:after,.resizing-details .details-resizer:after { background:var(--primary) }
.resizing-details { cursor:row-resize; user-select:none }
.cache-metric meter { display:block; width:100%; max-width:145px; height:5px; margin:10px 0 7px; border:0; border-radius:3px; background:var(--secondary) }
.cache-metric meter::-webkit-meter-bar { background:var(--secondary); border:0; border-radius:3px }
.cache-metric meter::-webkit-meter-optimum-value { background:var(--primary); border-radius:3px }
.cache-metric meter::-moz-meter-bar { background:var(--primary); border-radius:3px }
@media(max-width:760px) { .supervisor-resizer { display:none }.run-clock { display:none }.run-header { padding:0 10px; gap:5px }.run-heading { gap:6px }.blueprint-select { max-width:180px; font-size:11px }.run-actions { gap:4px } }


.supervisor { background:var(--background); }
.supervisor-heading { height:46px; }.supervisor-heading strong { font-size:15px; }.supervisor-heading .small-badge { font-size:11px; }
.supervisor>.panel-tabs { height:42px; flex-shrink:0; }.supervisor>.panel-tabs>button { font-size:13px; }.supervisor .count { font-size:11px; }
.supervisor :deep(.chat-column) { padding:20px 18px; gap:23px; }
.supervisor :deep(.chat-user-turn) { font-size:15px; line-height:25px; max-width:94%; }
.supervisor :deep(.md-body) { font-size:15px; line-height:26px; }
.supervisor :deep(.chat-notice) { font-size:12px; }
.pending-review { display:flex; align-items:center; gap:8px; padding:10px 17px; border-bottom:1px solid var(--divider); font-size:12px; color:var(--primary); background:var(--primary-soft); flex-shrink:0; text-align:left; }.pending-review svg:last-child { margin-left:auto; }
.supervision-records { flex:1; min-height:0; overflow:auto; padding:18px; font-size:14px; }.supervision-records h3 { font-size:16px; font-weight:600; line-height:1.5; margin:13px 0 8px; }.supervision-records p { font-size:14px; line-height:1.75; color:var(--muted-foreground); overflow-wrap:anywhere; }
.record-filter { display:flex; align-items:center; justify-content:space-between; margin-bottom:20px; color:var(--muted-foreground); font-size:12px; }.record-filter>div { display:flex; gap:3px; padding:2px; border:1px solid var(--border); border-radius:7px; }.record-filter button { padding:3px 9px; border-radius:4px; }.record-filter button.active { color:var(--foreground); background:var(--secondary); }
.record-meta { display:flex; flex-wrap:wrap; gap:6px; justify-content:space-between; color:var(--muted-foreground); font-size:11px; }.request-record { margin-bottom:24px; }.request-record+.request-record { padding-top:18px; border-top:1px solid var(--divider); }.request-record>p { margin:10px 0; }
.request-timeline { list-style:none; padding:6px 0; margin:13px 0; }.request-timeline li { position:relative; display:flex; gap:9px; align-items:center; font-size:12px; padding:7px 0; color:var(--muted-foreground); }.request-timeline li>span:last-child:not(.timeline-dot) { margin-left:auto; font-size:11px; }.request-timeline .complete>svg { color:#229d77; }.timeline-dot { width:7px; height:7px; margin:0 3px; border:1px solid var(--primary); border-radius:50%; }
.record-action { border:1px solid var(--border); border-radius:6px; padding:6px 9px; font-size:12px; }.context-reference { font-size:12px!important; }.review-section { border-top:1px solid var(--divider); margin-top:18px; padding-top:14px; }.review-section h4 { font-size:12px; font-weight:500; margin-bottom:7px; }.review-section button { display:flex; gap:7px; align-items:center; width:100%; text-align:left; padding:8px 0; color:var(--muted-foreground); font-size:13px; }.review-section button svg:last-child { margin-left:auto; }.review-record :deep(.review-proposal) { margin-top:20px; }
.supervision-composer { flex-shrink:0; padding:12px 14px 10px; border-top:1px solid var(--divider); }.supervision-composer .quick-prompts { gap:7px; margin-bottom:11px; flex-wrap:wrap; }.supervision-composer .quick-prompts button { font-size:12px; padding:5px 8px; }
.supervision-composer .chat-composer { border-radius:15px; }.supervision-composer .chat-composer-textarea { font-size:15px; line-height:24px; padding:13px 14px 5px; min-height:74px; }.supervision-composer .chat-composer-textarea:focus-visible { outline:none; }.supervision-composer .chat-composer-controls { gap:8px; padding:5px 10px 10px; flex-wrap:nowrap; }.supervision-composer .chat-send { margin-left:auto; border-radius:8px; width:30px; height:30px; }
.composer-model { font-size:12px; white-space:nowrap; }.composer-footnote { display:flex; justify-content:space-between; gap:8px; font-size:10px; color:var(--muted-foreground); margin:9px 2px 0; }.composer-footnote>span:last-child { white-space:nowrap; }.node-attachment { display:flex; gap:6px; align-items:center; margin:10px 12px 0; padding:5px 8px; border:1px solid var(--border); border-radius:6px; width:fit-content; font-size:12px; }.node-attachment button { display:grid; place-items:center; margin-left:4px; }.service-notice { margin:0 0 10px; font-size:12px; line-height:1.7; color:var(--muted-foreground); }.empty-message h3 { font-size:17px; font-weight:500; margin-bottom:8px; }.empty-message p { font-size:14px; line-height:1.8; }

</style>
