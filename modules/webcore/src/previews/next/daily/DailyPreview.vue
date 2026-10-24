<script setup lang="ts">
import { computed, reactive, ref } from 'vue'
import { ArrowLeft, ArrowUp, ChevronRight, FileCode, MessageSquare, Moon, Pause, Play, Sun, Workflow, X } from '@lucide/vue'
import { vDialogFocus } from '../dialog-focus'
import './daily.css'

const ids = ['WF-PV-01', 'WF-PV-02', 'WF-PV-03'] as const
type PreviewId = typeof ids[number]
const query = new URLSearchParams(location.search).get('preview')
const page = ref<PreviewId>(ids.includes(query as PreviewId) ? query as PreviewId : 'WF-PV-01')
const dark = ref(false)
const scenario = ref('Running')
const scenarios = ['Running', 'Starting', 'Paused', 'Waiting Approval', 'Completed', 'Failed', 'Stopped', 'Disconnected', 'Missing Model', 'Loading', 'Empty', 'Read Only', 'Permission Denied', 'Stale Version', 'Unknown Effect', 'Budget Exhausted', 'Policy Expired', 'Permission Revoked', 'Missing Source']
const session = ref('A')
const drafts = reactive<Record<string, string>>({ A: 'After this run, review the public API.', B: 'Investigate the search index.', supervision: 'Keep the existing error messages.', text: 'export const total = (a, b) => a + b\n', canvas: '{ "nodes": [ // unfinished edit', new: '' })
const backup = reactive<Record<string, string>>({})
const histories = reactive<Record<string, string[]>>({ A: ['Fix the validation bug across parser.ts and api.ts.'], B: ['Separate conversation · search investigation'], supervision: [] })
const supervision = ref(false)
const notice = ref('')
const pending = ref(false)
const control = ref('')
const dialog = ref('')
const editor = ref('text')
const baseline = 'export const total = (a, b) => a - b\n'
const disk = ref('export const total = (a, b) => Number(a) + Number(b)\n')
const diskRevision = ref(7)
const reviewedRevision = ref(7)
const resultInDraft = ref(false)
const binding = ref('Chat Handoff')
const endPolicy = ref('Keep Results')
const modelCalls = ref(0)
const turn = ref(0)
const blocked = computed(() => !['Completed', 'Failed', 'Stopped'].includes(scenario.value))
const unavailable = computed(() => ['Disconnected', 'Permission Denied', 'Read Only', 'Missing Model', 'Loading'].includes(scenario.value))
const key = computed(() => supervision.value ? 'supervision' : session.value)
const recoveryState = computed(() => scenario.value === 'Disconnected' ? 'Not Backed Up · Offline' : backup[editor.value] === drafts[editor.value] ? 'Backed Up on Host · Fixture' : 'Not Backed Up')
const needsDiagnosis = computed(() => ['Unknown Effect', 'Budget Exhausted', 'Policy Expired', 'Permission Revoked', 'Missing Source', 'Stale Version'].includes(scenario.value))
const reason = computed(() => ({
  'Budget Exhausted': 'Root budget is exhausted. A new task cannot reset this ledger.',
  'Policy Expired': 'The continuation policy expired. Prepare a plan with a new explicit scope.',
  'Permission Revoked': 'Permission was revoked. The old approval cannot be restored.',
  'Missing Source': 'The source conversation is unavailable. No replacement conversation is selected.',
  'Stale Version': 'Blueprint r4 differs from approved r3. Compare and confirm the current version.',
  'Unknown Effect': 'A command may have changed files. Inspect its journal before any further execution.',
  'Disconnected': 'Connection lost. Ownership and control outcomes are unknown until reconciliation.',
  'Waiting Approval': 'Waiting for your decision on one exact file operation.',
  'Missing Model': 'No supervision model is configured. Direct controls remain available.',
} as Record<string, string>)[scenario.value] ?? `The run is ${scenario.value.toLowerCase()}.`)
function theme() { dark.value = !dark.value; document.documentElement.classList.toggle('dark', dark.value) }
function select(id: PreviewId) { page.value = id; dialog.value = ''; notice.value = '' }
function submit(event?: KeyboardEvent) {
  if (event && (event.isComposing || event.keyCode === 229 || event.repeat || event.shiftKey)) return
  event?.preventDefault()
  if (pending.value || !drafts[key.value]?.trim() || unavailable.value || (!supervision.value && blocked.value)) return
  histories[key.value] ??= []
  histories[key.value]!.push(drafts[key.value]!)
  drafts[key.value] = ''
  turn.value++
  notice.value = 'Accepted · Fixture Receipt · No Model Call'
}
function direct(action: string) {
  if (scenario.value === 'Disconnected') { control.value = `${action} outcome unknown · Reconcile this run`; return }
  if (scenario.value === 'Permission Denied') { control.value = `${action} rejected · Permission denied`; return }
  if (scenario.value === 'Unknown Effect') { control.value = 'Inspect the journal before recovery'; return }
  scenario.value = action === 'Stop' ? 'Stopped' : action === 'Pause' ? 'Paused' : 'Running'
  control.value = `${action} confirmed for run wf-run-42 · Fixture`
}
function backedUp() { if (scenario.value === 'Disconnected') { notice.value = 'Offline · Your edits are not backed up'; return }; backup[editor.value] = drafts[editor.value]!; notice.value = 'Host ACK for this exact edit · Fixture' }
function recover() { if (backup[editor.value] === undefined) { notice.value = 'No host-confirmed recovery copy'; return }; drafts[editor.value] = backup[editor.value]!; notice.value = 'Recovery copy loaded into editor. Disk is unchanged; review the conflict.'; dialog.value = 'Compare Changes'; reviewedRevision.value = diskRevision.value }
function resolveConflict(mode: string) {
  if (mode === 'Use Disk') { drafts[editor.value] = disk.value; dialog.value = ''; return }
  if (mode === 'Keep Both') { notice.value = 'Created parser.recovered.ts using create-only · Fixture'; dialog.value = ''; return }
  if (reviewedRevision.value !== diskRevision.value) { notice.value = 'Conflict: disk changed again. Review the latest version.'; return }
  disk.value = drafts[editor.value]!; diskRevision.value++; dialog.value = ''; notice.value = 'Saved the reviewed content using a conditional write · Fixture'
}
function acceptResult() { drafts[session.value] = `${drafts[session.value]}\n\n[Result source: wf-run-42 / check-7 / revision 3]\nTests: 12 passed. Review parser.ts and api.ts.`; resultInDraft.value = true; notice.value = 'Added a source reference to the selected draft. Send remains explicit.' }
function reset() { scenario.value = 'Running'; supervision.value = false; control.value = ''; notice.value = ''; dialog.value = ''; resultInDraft.value = false }
</script>

<template>
  <div class="dw-shell">
    <header class="dw-preview-bar"><strong>M / METTEUR</strong><span class="dw-fixture">ISOLATED FIXTURE · r1</span><span class="dw-preview-note">No daemon, user files or model connected</span><button class="btn" :aria-label="dark ? 'Light Theme' : 'Dark Theme'" @click="theme"><Sun v-if="dark" :size="16"/><Moon v-else :size="16"/></button></header>
    <div class="dw-review-controls"><nav aria-label="Preview Groups"><button v-for="(id,i) in ids" :key="id" class="btn" :class="{'btn-primary':page===id}" @click="select(id)">{{ id }} · {{ ['Chat & Supervision','Drafts & Files','Tasks & Results'][i] }}</button></nav><label>Fixture State <select v-model="scenario" class="input" aria-label="Fixture State"><option v-for="item in scenarios" :key="item">{{ item }}</option></select></label><button class="btn" @click="reset">Reset State</button></div>
    <main class="dw-workbench">
      <aside class="dw-sidebar"><div class="dw-project"><span class="dw-project-icon">M</span><div><strong>Metrics</strong><small>Synthetic Workspace</small></div></div><nav aria-label="Workspace"><button :class="{selected:page==='WF-PV-01'}" @click="select('WF-PV-01')"><MessageSquare :size="17"/>Chat</button><button :class="{selected:page==='WF-PV-02'}" @click="select('WF-PV-02')"><FileCode :size="17"/>Explorer</button><button :class="{selected:page==='WF-PV-03'}" @click="select('WF-PV-03')"><Workflow :size="17"/>Tasks</button></nav><p class="dw-section-label">CONVERSATIONS</p><button v-for="id in ['A','B']" :key="id" class="dw-session" :class="{selected:session===id}" @click="session=id;supervision=false;select('WF-PV-01')">{{ id === 'A' ? 'Validation Repair' : 'Search Investigation' }}<small>Session {{ id }}</small></button><p class="dw-sidebar-foot">Drafts stay with their own session.<br/>Run completion keeps this page open.</p></aside>

      <section v-if="page==='WF-PV-01'" class="dw-panel">
        <header class="dw-page-header"><div><p class="dw-section-label">{{ supervision ? 'EXECUTION / SUPERVISION' : `CHAT / SESSION ${session}` }}</p><h1>{{ supervision ? 'Validation Repair · Supervision' : session === 'A' ? 'Validation Repair' : 'Search Investigation' }}</h1></div><button v-if="supervision" class="btn" @click="supervision=false"><ArrowLeft :size="15"/>Back to Chat</button><span v-else class="dw-tag">Original Conversation</span></header>
        <div class="dw-run-card"><div class="dw-run-title"><Workflow :size="17"/><strong>Repair Validation Across Two Files</strong><span class="dw-tag">{{ scenario }}</span></div><p>{{ reason }}</p><div class="dw-inline-actions"><button v-if="!supervision" class="btn" @click="supervision=true">Open Supervision<ChevronRight :size="14"/></button><button class="btn" @click="select('WF-PV-03')">View Run</button><button class="btn" :disabled="['Completed','Stopped','Failed'].includes(scenario)" @click="direct(scenario==='Paused'?'Resume':'Pause')"><Play v-if="scenario==='Paused'" :size="13"/><Pause v-else :size="13"/>{{scenario==='Paused'?'Resume':'Pause'}}</button><button class="btn dw-stop" :disabled="['Completed','Stopped','Failed'].includes(scenario)" @click="dialog='Stop Run'">Stop</button></div><p v-if="control" role="status">{{control}}</p></div>
        <div v-if="scenario==='Loading'" class="dw-empty">Loading the selected conversation…</div><div v-else-if="scenario==='Empty'" class="dw-empty">What Should We Work On?</div><div v-else class="dw-transcript" role="log" aria-label="Preview Conversation"><article v-for="(text,i) in histories[key]" :key="i" class="dw-message"><span class="dw-avatar">{{supervision?'Y':'Y'}}</span><div><strong>You</strong><p>{{text}}</p></div></article><article class="dw-message"><span class="dw-avatar dw-agent">M</span><div><strong>{{supervision?'Concierge':'Metteur'}}</strong><p>{{supervision?'This channel can inspect the run and forward requests. A request is not an approval. Direct Pause and Stop work without a supervision model.':'The stage blueprint is ready. Its run is separate from this conversation. You can keep reading and writing your next draft here.'}}</p><span class="dw-subtle">{{supervision?'Run wf-run-42 · Conversation supervision-42':'Blueprint r3 · Source session A · Handoff endpoint turn-6'}}</span></div></article></div>
        <div class="dw-composer"><p v-if="!supervision && blocked" class="dw-muted">{{scenario==='Disconnected'?'Run ownership is unknown. Reconnect to reconcile before sending.':'Blueprint occupies this workspace. Add current-run requirements in supervision.'}}</p><p v-if="supervision && scenario==='Missing Model'" class="dw-muted">Configure a supervision model in Settings. Direct controls remain available.</p><textarea v-model="drafts[key]" aria-label="Preview Message" :placeholder="supervision?'Ask about this run or submit a request':'Keep a draft for your next message'" @keydown.enter="submit"/><div class="dw-composer-footer"><span>{{scenario==='Disconnected'?'Not Backed Up · Offline':'Draft belongs to '+(supervision?'this run':'session '+session)}}</span><button class="btn btn-primary" aria-label="Send Preview Message" :disabled="unavailable || (!supervision && blocked) || !drafts[key]?.trim() || pending" @click="submit()"><ArrowUp :size="16"/>{{supervision?'Send Request':'Send'}}</button></div></div>
      </section>

      <section v-else-if="page==='WF-PV-02'" class="dw-panel">
        <header class="dw-page-header"><div><p class="dw-section-label">EXPLORER / RECOVERY</p><h1>Keep Your Work</h1><p class="dw-muted">A recovery copy is separate from the saved file and approved run version.</p></div><span class="dw-tag">{{recoveryState}}</span></header>
        <div class="dw-tabs" role="tablist" aria-label="Draft Kind"><button v-for="kind in ['text','canvas','A','supervision']" :key="kind" role="tab" :aria-selected="editor===kind" @click="editor=kind">{{{text:'parser.ts',canvas:'repair.blueprint',A:'Chat Draft',supervision:'Supervision Draft'}[kind]}}</button></div>
        <div class="dw-editor-heading"><FileCode :size="16"/><strong>{{editor==='text'?'src/parser.ts':editor==='canvas'?'blueprints/repair.blueprint':editor==='A'?'Session A':'Run wf-run-42'}}</strong><span class="dw-tag">Window 1 · Independent Copy</span></div><textarea v-model="drafts[editor]" class="dw-source-editor" aria-label="Recovery Editor" spellcheck="false"/><p class="dw-muted">{{editor==='canvas'?'Invalid JSON is still recoverable. Formal Save validates the blueprint.':'Edits do not change the saved file until you choose Save.'}}</p>
        <div class="dw-inline-actions"><button class="btn" @click="backedUp">Back Up Draft</button><button class="btn" @click="recover">Reopen Confirmed Copy</button><button class="btn" @click="reviewedRevision=diskRevision;dialog='Compare Changes'">Compare Changes</button><button class="btn" @click="dialog='Reference File'">Reference in Chat</button><button class="btn dw-stop" @click="dialog='Discard Draft'">Discard Draft</button></div>
        <section class="dw-evidence-section"><h2>Recovery Copies</h2><div class="dw-recovery-row"><div><strong>Window 1 · src/parser.ts</strong><p>{{backup.text===undefined?'No confirmed backup yet':'Host-confirmed copy · Ready to recover'}}</p></div><button class="btn" @click="editor='text';recover()">Review</button></div><div class="dw-recovery-row"><div><strong>Window 2 · src/parser.ts</strong><p>Separate copy · Disk has changed · Fixture</p></div><button class="btn" @click="notice='Window 2 copy remains separate. Choose a copy explicitly.'">Inspect Copy</button></div><div class="dw-recovery-row"><div><strong>Deleted Session · Orphaned Draft</strong><p>Manual copy available · Does not recreate the conversation</p></div><button class="btn" @click="notice='Orphaned text copied to current draft; no session was created.';drafts[session]='Recovered orphan text';">Copy Text</button></div></section>
        <p class="dw-muted">Prompt selections: Personal / Repair r2 → Project / Repair r5 → Personal / Repair r2. Recovery keeps order and repeats; send rechecks access and revisions. Permissions require current configuration.</p>
      </section>

      <section v-else class="dw-panel">
        <header class="dw-page-header"><div><p class="dw-section-label">TASKS / BLUEPRINT RUN</p><h1>Validation Repair</h1><p class="dw-muted">Execution, deterministic checks and your acceptance are separate.</p></div><span class="dw-tag">{{scenario}}</span></header>
        <div class="dw-run-options"><label>Launch Source<select v-model="binding" class="input" aria-label="Launch Source"><option>Chat Handoff</option><option>Editor Direct Run</option></select></label><label>After Completion<select v-model="endPolicy" class="input" aria-label="Completion Policy"><option>Keep Results</option><option>Continue Source Conversation</option></select></label><button class="btn" @click="dialog='Review Launch'">Review Launch</button></div>
        <div class="dw-run-card"><div class="dw-run-title"><Workflow :size="17"/><strong>{{scenario==='Completed'?'Execution Completed':scenario==='Stopped'?'Cancelled by You':'Implement → Test → Report'}}</strong><span class="dw-tag">wf-run-42</span></div><p>{{reason}}</p><p>{{binding==='Editor Direct Run'?'Independent Run · No source conversation · No automatic continuation':`Source Session A · Launch Intent launch-17 · ${endPolicy==='Keep Results'?'Results only; no new model turn':'Continue in session A once policy and revision checks pass'}`}}</p><div class="dw-inline-actions"><button class="btn" @click="supervision=true;select('WF-PV-01')">Open Supervision</button><button class="btn" :disabled="needsDiagnosis || ['Stopped','Completed','Failed'].includes(scenario)" @click="direct('Resume')">Resume Execution</button><button class="btn dw-stop" :disabled="['Stopped','Completed','Failed'].includes(scenario)" @click="dialog='Stop Run'">Stop</button><button v-if="needsDiagnosis || scenario==='Disconnected'" class="btn" @click="dialog='Recovery Diagnostics'">Inspect Reason & Evidence</button><button v-if="scenario==='Waiting Approval'" class="btn" @click="dialog='Review Exact Action'">Review Exact Action</button></div></div>
        <section class="dw-result-grid" aria-label="Result Evidence"><article><span class="dw-section-label">EXECUTION</span><h2>{{scenario==='Completed'?'Completed':scenario}}</h2><p>Run state comes from the host.</p></article><article><span class="dw-section-label">DETERMINISTIC CHECKS</span><h2>{{scenario==='Completed'?'12 Passed':'Not Run / Pending'}}</h2><button class="dw-link" @click="notice='Fixture check-7: node --test · exit 0 · parser.test.ts · source revision 3'">Open Test Report</button></article><article><span class="dw-section-label">MODEL SUMMARY</span><h2>{{scenario==='Completed'?'Fix Proposed':'Unavailable'}}</h2><p>Opinion; inspect actual files and checks.</p></article><article><span class="dw-section-label">YOUR ACCEPTANCE</span><h2>Not Reviewed</h2><p>A successful run does not mark this approved.</p></article></section>
        <section class="dw-evidence-section"><h2>Files & Artifacts</h2><button class="dw-evidence-link" @click="notice='Open immutable diff: src/parser.ts @ revision 3'">src/parser.ts <span>Revision 3 · View Diff</span></button><button class="dw-evidence-link" @click="notice='Open immutable diff: src/api.ts @ revision 3'">src/api.ts <span>Revision 3 · View Diff</span></button><button class="dw-evidence-link" @click="notice='Fixture artifact: test-report.txt · check-7'">test-report.txt <span>check-7 · View Report</span></button></section>
        <div class="dw-inline-actions"><label>Target Conversation<select v-model="session" class="input" aria-label="Result Target"><option>A</option><option>B</option></select></label><button class="btn btn-primary" @click="acceptResult">Add Result to Draft</button><span class="dw-muted">{{resultInDraft?'Source reference added · Send is still explicit':'Adds source data; does not run the model'}}</span></div><details class="dw-details"><summary>Identity, Budget & Unhandled Requests</summary><p>Chat plan: 3 todos · Persistent task: task-17 · Blueprint run: wf-run-42</p><p>{{binding==='Editor Direct Run'?'Source session: unavailable / independent run':'Source session: A · Source turn: turn-6 · Launch revision: 3'}}</p><p>Usage: unknown until provider reports it · Root ledger preserved</p><p>Unhandled request req-9: run ended before forwarding · Copy remains in supervision history</p><p>Fixture sends: {{turn}} · Actual model calls: {{modelCalls}}</p></details>
      </section>
    </main>
    <p v-if="notice" class="dw-notice" role="status">{{notice}}<button class="btn" aria-label="Dismiss Notice" @click="notice=''">×</button></p>
    <footer class="dw-footer">{{page}} / r1 · dw-fixture-1 · User Approval Pending · All actions simulate fixture state only</footer>
    <div v-if="dialog" class="dw-overlay" @keydown.esc="dialog=''" @click.self="dialog=''">
      <section v-dialog-focus role="dialog" aria-modal="true" :aria-label="dialog" class="dw-dialog"><header><h2>{{dialog}}</h2><button class="btn" aria-label="Close Dialog" @click="dialog=''"><X :size="16"/></button></header>
        <template v-if="dialog==='Compare Changes'"><p>Compare your recovery draft, the common baseline and the latest saved file. Saving remains conditional on this disk version.</p><div class="dw-three-way"><div><h3>Common Baseline</h3><pre>{{baseline}}</pre></div><div><h3>Your Draft</h3><pre>{{drafts[editor]}}</pre></div><div><h3>Latest Disk · r{{diskRevision}}</h3><pre>{{disk}}</pre></div></div><div class="dw-inline-actions"><button class="btn" @click="diskRevision++;disk+='// external change\n'">Simulate External Change</button><button class="btn" @click="reviewedRevision=diskRevision;notice='Latest disk version reviewed'">Review Latest</button></div><div class="dw-inline-actions"><button class="btn" @click="resolveConflict('Use Disk')">Use Disk</button><button class="btn" @click="resolveConflict('Keep Both')">Keep Both</button><button class="btn btn-primary" @click="resolveConflict('Save')">Save Reviewed Content</button></div></template>
        <template v-else-if="dialog==='Reference File'"><p>src/parser.ts has unsaved edits. Choose what the next message references.</p><p>Saved version: r{{diskRevision}} · content identity verified by host · Fixture</p><div class="dw-inline-actions"><button class="btn" @click="notice='Attached saved revision '+diskRevision+'; editor draft unchanged';dialog=''">Use Saved Version</button><button class="btn btn-primary" @click="reviewedRevision=diskRevision;dialog='Compare Changes'">Save Then Reference</button></div><p>Later tool reads observe the file version at read time.</p></template>
        <template v-else-if="dialog==='Discard Draft'"><p>Discard this exact recovery copy? Other windows and the saved file are unchanged. A tombstone rejects late backups for this draft.</p><div class="dw-inline-actions"><button class="btn" @click="dialog=''">Keep Editing</button><button class="btn dw-stop" @click="delete backup[editor];drafts[editor]='';notice='Draft discarded · Late fixture writes rejected';dialog=''">Discard This Copy</button></div></template>
        <template v-else-if="dialog==='Stop Run'"><p>Stop run wf-run-42? This cancels its generation and automatic continuation. Files already written remain available for review. Unhandled supervision requests remain visible.</p><div class="dw-inline-actions"><button class="btn" @click="dialog=''">Keep Running</button><button class="btn dw-stop" @click="direct('Stop');dialog=''">Stop This Run</button></div></template>
        <template v-else-if="dialog==='Review Launch'"><p>Goal: fix validation across parser.ts and api.ts, then run the tests.</p><dl class="dw-facts"><dt>Blueprint</dt><dd>repair.blueprint · r3 · exact digest</dd><dt>Context</dt><dd>Personal / Repair r2, Project / Repair r5 · ordered frozen snapshots</dd><dt>Source</dt><dd>{{binding==='Editor Direct Run'?'Independent run; no source conversation':'Session A through turn-6 only'}}</dd><dt>Completion</dt><dd>{{binding==='Editor Direct Run'?'Results only; no automatic continuation':endPolicy}}</dd></dl><p v-if="scenario==='Stale Version'" role="alert">Version changed. Compare and create a new proposal.</p><button class="btn btn-primary" :disabled="['Stale Version','Disconnected','Unknown Effect','Permission Denied'].includes(scenario)" @click="notice='One fixture launch intent accepted. This does not start a real run.';dialog=''">Confirm This Version</button></template>
        <template v-else-if="dialog==='Review Exact Action'"><p>Write src/parser.ts · expected saved revision {{diskRevision}} · requested content digest fixture-93f</p><pre>{{drafts.text}}</pre><p>Applies only to this request and content. Chat text cannot approve it.</p><div class="dw-inline-actions"><button class="btn" @click="notice='Exact action rejected · Fixture';dialog=''">Reject</button><button class="btn btn-primary" @click="notice='Exact action approval recorded · Fixture';dialog=''">Approve This Action</button></div></template>
        <template v-else><p>{{reason}}</p><p>Journal: command-17 · Receipt: effect-9 · Ownership: requires reconciliation.</p><p>No generic Continue action is available. Inspect immutable file changes, export the journal, or prepare a new version-bound proposal where permitted.</p><div class="dw-inline-actions"><button class="btn" @click="notice='Fixture journal opened; no effects replayed'">View Journal</button><button class="btn" @click="notice='Fixture recovery report prepared; no execution or authorization'">Export Report</button></div></template>
        <p class="dw-muted">Isolated fixture · No real execution, permission or product approval</p>
      </section>
    </div>
  </div>
</template>
