<script setup lang="ts">
import { vDialogFocus } from './dialog-focus'
import TaskStartDialog from '@/components/continuations/TaskStartDialog.vue'
import { computed, ref, watch } from 'vue'
import { Pause, Play, Square, Terminal, GitBranch, ArrowUpRight, ShieldCheck, AlertTriangle } from '@lucide/vue'
const props = defineProps<{ scenario: string }>()
const starting=ref(false),fixtureGoal=ref('')
const state=ref('Waiting'), selected=ref('Build report'), stopDialog=ref(false), notice=ref('')
watch(()=>props.scenario,s=>{state.value=s==='Ready'?'Waiting':s;notice.value=''}, {immediate:true})
const locked=computed(()=>['Disconnected','Read only','Permission denied','Loading'].includes(state.value))
const terminal=computed(()=>state.value==='Stopped')
function pause(){state.value=state.value==='Paused'?'Waiting':'Paused';notice.value=state.value==='Paused'?'New automatic turns are paused. An active turn would finish normally.':'Policy, budget and session revision will be checked before resuming.'}
function stop(){state.value='Stopped';stopDialog.value=false;notice.value='Future triggers invalidated. Only this task’s sources receive cancellation; unresolved effects remain visible.'}
</script>
<template>
<section class="nx-preview"><div class="pv-heading nx-heading"><div><p class="pv-eyebrow">PV-03 / REVISION 2</p><h1>Task continuation</h1><p class="pv-muted">A turn can finish while its task waits for a result.</p></div><div class="nx-icon-label"><span class="pv-badge">{{ state }}</span><button class="btn btn-primary" @click="starting=true">New task</button></div></div>
<div v-if="state==='Empty'" class="pv-empty">No waiting tasks. Tasks appear here after an authorized wait is saved.</div>
<div v-else class="nx-task-layout"><aside class="nx-task-list"><button class="pv-row" :class="{selected:selected==='Build report'}" @click="selected='Build report'"><span class="nx-icon-label"><Terminal :size="16"/><strong>Build report</strong></span><span class="pv-muted">Current session · {{ state }}</span></button><button class="pv-row" :class="{selected:selected==='Read-only review'}" @click="selected='Read-only review'"><span class="nx-icon-label"><GitBranch :size="16"/><strong>Read-only review</strong></span><span class="pv-muted">Child task · Results ready</span></button></aside>
<div class="nx-task-main"><div class="pv-between"><h2>{{ selected }}</h2><button class="btn" @click="notice='Fixture: opening the owning session does not start a new turn.'">Open session <ArrowUpRight :size="14"/></button></div>
<div v-if="['Needs attention','Conflict','Error','Budget exhausted','Permission denied'].includes(state)" class="nx-alert"><AlertTriangle :size="17"/><span>{{ state==='Conflict'?'A newer user message changed this session. The old task will not overwrite it.':state==='Budget exhausted'?'No more automatic turns can start within the current budget.':'Automatic continuation is blocked. Review the source and policy before continuing.' }}</span></div>
<div v-if="state==='Disconnected'" class="nx-alert">Connection unavailable. Showing the last saved state; controls are disabled.</div>
<div class="nx-progress"><span class="complete">Turn saved</span><span>→</span><span class="current">{{ state }}</span><span>→</span><span>Check policy</span><span>→</span><span>New turn</span></div>
<div class="nx-source"><Terminal :size="22"/><div><strong>{{selected==='Build report'?'Local command · job-18':'Read-only child · child-04'}}</strong><p class="pv-muted">{{selected==='Build report'?'Waiting for any terminal result, including failure.':'Bounded result available; no write tools granted.'}}</p></div><span class="pv-badge">{{selected==='Build report'?'Running':'Succeeded'}}</span></div>
<dl class="nx-details"><dt>When ready</dt><dd>Summarize the result in the same session</dd><dt>Waiting policy</dt><dd>Any terminal result · deadline in 20 minutes</dd><dt>Remaining budget</dt><dd>3 automatic turns · 12,000 tokens · cost unavailable</dd><dt>Permission scope</dt><dd class="nx-icon-label"><ShieldCheck :size="15"/>Existing permissions only · approvals rechecked</dd><dt>Background effects</dt><dd>{{selected==='Build report'?'Potential writes: new conflicting execution is held until the command ends.':'Read-only allowlist enforced by the host.'}}</dd></dl>
<details class="nx-disclosure"><summary>Source, frozen parameters and history</summary><pre>{ "next_step": "summarize_validation" }</pre><p class="pv-muted">Task task-07 · generation 3 · session revision 12</p><p>Output is source data. It cannot approve actions.</p><p>10:31 · Wait saved<br/>10:31 · Foreground slot released</p></details>
<div class="pv-actions"><button class="btn" :disabled="locked||terminal" @click="pause"><Play v-if="state==='Paused'" :size="15"/><Pause v-else :size="15"/>{{state==='Paused'?'Resume automatic continuation':'Pause automatic continuation'}}</button><button class="btn nx-danger" :disabled="locked||terminal" @click="stopDialog=true"><Square :size="14"/>Stop task</button></div><p v-if="notice" role="status" class="nx-feedback">{{notice}}</p>
</div></div>
<div v-if="stopDialog" class="pv-overlay" @click.self="stopDialog=false" @keydown.esc="stopDialog=false"><section v-dialog-focus role="dialog" aria-modal="true" aria-label="Stop task" class="pv-modal"><h2>Stop this task?</h2><p>Cancel future triggers and request cancellation of this task’s command and child tasks. Other tasks are unaffected. Unknown write effects remain blocked for review.</p><div class="pv-actions"><button class="btn" autofocus @click="stopDialog=false">Keep task</button><button class="btn nx-danger" @click="stop">Stop task</button></div></section></div>
<TaskStartDialog v-if="starting" :models="['Local fixture model']" @close="starting=false" @start="input=>{fixtureGoal=input.goal;starting=false;state='Active';notice='Fixture: a new task session would start with the exact confirmed limits. No model was called.'}"/>
<p v-if="fixtureGoal" class="nx-feedback">New fixture goal: {{fixtureGoal}}</p>
</section>
</template>
