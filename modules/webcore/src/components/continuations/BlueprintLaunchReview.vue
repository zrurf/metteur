<script setup lang="ts">
import InlineHelp from '@/components/resources/InlineHelp.vue'
import {computed,onBeforeUnmount,ref,watch} from 'vue'
import {Workflow,Layers,MessagesSquare,ShieldCheck,X} from '@lucide/vue'
import {gateway} from '@/core'
import {useWorkspaceStore} from '@/stores/workspace'
import {vDialogFocus} from '@/components/resources/dialog-focus'
interface Proposal {
 intent_id:string;digest:string;blueprint_name:string
 manifest:Record<string,unknown>
 resource_versions?:{name:string;reference:{scope:unknown;revision:number;content_digest:string}}[]
 handoff:{goal:string;constraints:string[];model_summary:string;message_end:number;allow_source_read:boolean}
 policy:{max_turns:number;token_limit:number;expires_ms:number}
 resume_on_completion:boolean
}
const props=defineProps<{payload:Proposal;requestId:string}>()
const emit=defineEmits<{resolved:[]}>()
const workspace=useWorkspaceStore(),section=ref('overview'),resume=ref(false),source=ref(false),confirmed=ref(false),busy=ref(false),error=ref('')
let generation=0
watch([()=>props.requestId,()=>props.payload,()=>workspace.active?.path],()=>{generation++;busy.value=false;resume.value=props.payload.resume_on_completion;source.value=props.payload.handoff.allow_source_read;confirmed.value=false;section.value='overview';error.value=''}, {immediate:true,deep:true,flush:'sync'})
watch([resume,source],()=>{confirmed.value=false}, {flush:'sync'})
onBeforeUnmount(()=>{generation++})
const tabs=[{id:'overview',label:'Run overview',icon:Workflow},{id:'resources',label:'Bound versions',icon:Layers},{id:'handoff',label:'Conversation handoff',icon:MessagesSquare}]
const resourceRows=computed(()=>{
 const m=props.payload.manifest
 const file=m.file_version as {blueprint_uri?:string;blob_hash?:string}|undefined
 const rows=[{name:props.payload.blueprint_name,scope:file?.blueprint_uri??'Saved blueprint',digest:file?.blob_hash??String(m.graph_digest??'')}]
 for(const item of props.payload.resource_versions??[]) rows.push({name:item.name,scope:'Prompt · revision '+item.reference.revision,digest:item.reference.content_digest})
 for(const [name,digest] of Object.entries((m.functions??{}) as Record<string,string>))rows.push({name,scope:'Function',digest})
 for(const [name,digest] of Object.entries((m.addons??{}) as Record<string,string>))rows.push({name:name.includes('::')?name.slice(name.lastIndexOf('::')+2):name,scope:name.startsWith('global::')?'Global addon':'Workspace addon',digest})
 rows.push({name:'Node catalog',scope:'Host catalog',digest:String(m.node_catalog_digest??'')},{name:'Tool catalog',scope:'Host catalog',digest:String(m.tool_catalog_digest??'')})
 return rows
})
function display(value:unknown){return typeof value==='string'?value:JSON.stringify(value,null,2)}
async function decide(allow:boolean){
 const root=workspace.active?.path
 if(!root||busy.value||allow&&!confirmed.value)return
 const ticket=generation,current=()=>ticket===generation&&workspace.active?.path===root
 busy.value=true;error.value=''
 try{
  const result=await gateway.resourceRequest('confirmBlueprintLaunch',root,{request_id:props.requestId,intent_id:props.payload.intent_id,original_digest:props.payload.digest,allow,resume_on_completion:resume.value,allow_source_read:source.value})
  if(!current())return
  if(result.ok)emit('resolved');else error.value=result.error
 }catch(e){if(current())error.value=e instanceof Error?e.message:'The decision could not be recorded. Reconnect and review the pending request.'}
 finally{if(current())busy.value=false}
}
</script>
<template>
<div v-dialog-focus class="launch-review glass" role="dialog" aria-modal="true" aria-labelledby="launch-review-title">
 <header><div><h1 id="launch-review-title">Review a blueprint run <InlineHelp text="This approval applies to one version and one run." /></h1></div><button class="btn-icon" aria-label="Decline launch" :disabled="busy" @click="decide(false)"><X :size="18"/></button></header>
 <nav aria-label="Run review sections"><button v-for="tab in tabs" :key="tab.id" class="btn" :class="{'btn-primary':section===tab.id}" :aria-pressed="section===tab.id" @click="section=tab.id"><component :is="tab.icon" :size="16"/>{{tab.label}}</button></nav>
 <div class="launch-body">
  <template v-if="section==='overview'">
   <div class="launch-title"><Workflow :size="24"/><h2>{{payload.blueprint_name}}</h2></div>
   <div class="launch-columns"><div><h3>What this run will do</h3><p>{{payload.handoff.goal}}</p><dl><dt>Source boundary</dt><dd>Original conversation · through message {{payload.handoff.message_end}}</dd><dt>Execution scope</dt><dd>Current workspace · existing permission checks</dd><dt>Version binding</dt><dd><code>{{payload.digest}}</code></dd></dl></div>
   <aside><ShieldCheck :size="20"/><h3>Completion policy</h3><label><input v-model="resume" type="checkbox" :disabled="busy"/>Resume this conversation when the run finishes</label><p v-if="resume">Shared task limits: {{payload.policy.max_turns}} continuation turns · {{payload.policy.token_limit.toLocaleString()}} tokens.</p><p v-else>Keep the result for review without an automatic model turn.</p><p>Expires {{new Date(payload.policy.expires_ms).toLocaleString(undefined,{timeZoneName:'short'})}}. Stopping the run waits for your decision; it never restarts automatically.</p></aside></div>
  </template>
  <template v-else-if="section==='resources'">
   <h2>Versions you are approving <InlineHelp text="Changes invalidate this proposal." /></h2>
   <div class="versions-table"><table><thead><tr><th>Resource</th><th>Scope / version</th><th>Binding</th></tr></thead><tbody><tr v-for="row in resourceRows" :key="row.scope+row.name"><td>{{row.name}}</td><td>{{row.scope}}</td><td><code :title="row.digest">{{row.digest.slice(0,12)}}…</code></td></tr></tbody></table></div>
   <details><summary>Complete binding manifest</summary><pre>{{display(payload.manifest)}}</pre></details>
  </template>
  <template v-else>
   <h2>What crosses into execution</h2>
   <div class="launch-columns"><div><h3>Goal and constraints</h3><p>{{payload.handoff.goal}}</p><ul><li v-for="item in payload.handoff.constraints" :key="item">{{item}}</li></ul><h3>Model summary <InlineHelp text="Model-authored context does not grant permissions." /></h3><p>{{payload.handoff.model_summary || 'No model summary supplied.'}}</p></div><aside><h3>Source access</h3><label><input v-model="source" type="checkbox" :disabled="busy"/>Allow bounded reading through message {{payload.handoff.message_end}}</label><p>Original roles remain visible. Later messages and other conversations are excluded.</p></aside></div>
  </template>
 </div>
 <footer><p v-if="error" class="text-danger" role="alert">{{error}}</p><label><input v-model="confirmed" type="checkbox" :disabled="busy||!gateway.connected.value"/>I approve this version, handoff scope and completion policy</label><div class="footer-actions"><div><button class="btn" :disabled="busy" @click="decide(false)">Decline</button><button class="btn btn-primary" :disabled="busy||!confirmed||!gateway.connected.value" @click="decide(true)">{{busy?'Recording decision…':'Confirm and run'}}</button></div></div></footer>
</div>
</template>
<style scoped>
.launch-review{width:min(960px,100%);max-height:90dvh;overflow:auto;border:1px solid var(--border);border-radius:16px;color:var(--foreground)}
header{display:flex;align-items:flex-start;justify-content:space-between;padding:24px 28px}h1{font-size:23px;font-weight:650}h2{font-size:19px;font-weight:600}h3{font-size:14px;font-weight:600;margin:14px 0}p,li{font-size:13px;line-height:1.8;margin:9px 0;overflow-wrap:anywhere}header p,.muted,dt{color:var(--muted-foreground)}nav{display:flex;gap:8px;flex-wrap:wrap;padding:12px 28px;border-block:1px solid var(--border)}button,.launch-title{display:inline-flex;align-items:center;gap:9px}.launch-body{padding:24px 28px;min-height:280px}.launch-title{margin-bottom:18px}.launch-columns{display:grid;grid-template-columns:minmax(0,1.4fr) minmax(230px,1fr);gap:28px}aside{background:var(--surface-muted);border-radius:12px;padding:20px}dl{display:grid;grid-template-columns:130px minmax(0,1fr);gap:16px;font-size:12px;margin-top:20px}dd,code{overflow-wrap:anywhere}label{display:flex;gap:10px;align-items:flex-start;font-size:13px;line-height:1.65}input{margin-top:4px;accent-color:var(--primary)}footer{padding:20px 28px;border-top:1px solid var(--border)}.footer-actions{display:flex;justify-content:space-between;align-items:center;gap:18px;flex-wrap:wrap;margin-top:16px;font-size:12px}.footer-actions>div{display:flex;gap:10px}details{border-bottom:1px solid var(--border);padding:14px 0;font-size:13px}summary{cursor:pointer;text-transform:capitalize}pre{white-space:pre-wrap;overflow-wrap:anywhere;font-size:11px;padding:12px 0}.versions-table{overflow:auto}table{width:100%;border-collapse:collapse;font-size:13px;margin:16px 0}th,td{text-align:left;padding:12px;border-bottom:1px solid var(--border);overflow-wrap:anywhere}th{font-weight:500;color:var(--muted-foreground)}.resource-list>div{display:flex;justify-content:space-between;gap:12px;font-size:13px;padding:12px 0}ul{padding-left:18px;list-style:disc}@media(max-width:700px){.launch-columns{grid-template-columns:1fr}header,nav,.launch-body,footer{padding:18px}.resource-list>div{flex-direction:column}h1{font-size:20px}}
</style>
