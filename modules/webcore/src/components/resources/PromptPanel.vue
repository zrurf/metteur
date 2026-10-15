<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { vDialogFocus } from './dialog-focus'
import { gateway } from '@/core'
import { useWorkspaceStore } from '@/stores/workspace'
import { usePromptResourcesStore } from '@/stores/prompt-resources'
import { useBlueprintStore } from '@/stores/blueprint'
import { useChatStore } from '@/stores/chat'
import type { PromptEntry, PromptSnapshot } from '@/core/resources'
import './resources.css'
const workspace=useWorkspaceStore(), prompts=usePromptResourcesStore(), blueprint=useBlueprintStore(), chatStore=useChatStore()
type Fragment={id:string;scope:string;name:string;revision:number;body:string;raw?:PromptEntry;selectionId?:string}
const scope=ref('Project'),selection=ref(''),resources=ref<Fragment[]>([]),notice=ref(''),editing=ref(false),draft=ref(''),deleting=ref(false),previewBody=ref<Fragment|null>(null),busy=ref(false),failed=ref(false)
const nodes=ref<Fragment[]>([]),sent=ref<Fragment[]>([])
let generation=0
const shown=computed(()=>resources.value.filter(item=>item.scope===scope.value))
const selected=computed(()=>shown.value.find(item=>item.id===selection.value)??shown.value[0])
const chat=computed(()=>prompts.selections.map(item=>fromSnapshot(item.snapshot)))
const scenario=computed(()=>!gateway.connected.value?'Disconnected':busy.value?'Loading':failed.value?'Error':'Ready')
const writable=computed(()=>gateway.connected.value&&!busy.value&&!failed.value)
function fromEntry(entry:PromptEntry):Fragment{return{id:entry.reference.id,scope:entry.reference.scope.scope==='project'?'Project':'Personal',name:entry.content.header.name,revision:entry.reference.revision,body:entry.content.body,raw:entry}}
function fromSnapshot(snapshot:PromptSnapshot):Fragment{return{id:snapshot.reference.id,scope:snapshot.reference.scope.scope==='project'?'Project':'Personal',name:snapshot.name,revision:snapshot.reference.revision,body:snapshot.body,selectionId:snapshot.id}}
async function load(){
 const path=workspace.active?.path;if(!path)return;const ticket=++generation;busy.value=true;failed.value=false
 const result=await gateway.resourceRequest('queryPromptResources',path,{operation:'list',scope:scope.value.toLowerCase(),limit:50,after:null})
 if(ticket!==generation||workspace.active?.path!==path)return
 busy.value=false;failed.value=!result.ok
 if(!result.ok){notice.value=result.error;return}
 resources.value=(result.data as PromptEntry[]).map(fromEntry)
}
watch([()=>workspace.active?.path,scope],([path],[oldPath])=>{if(path!==oldPath){nodes.value=[];sent.value=[];notice.value=''}resources.value=[];editing.value=false;deleting.value=false;void load()},{immediate:true})
async function insert(target:'chat'|'blueprint',fragment=selected.value){
 if(!fragment?.raw||!writable.value)return
 try{
  if(target==='chat'){await prompts.addChat(fragment.raw.reference);notice.value='Selected as user-level content. Open Chat to compose a message, or send the selected instructions here.'}
  else{
   const file=blueprint.currentFile, graph=blueprint.uuidFor(file);if(!file||!graph)throw new Error('Open a blueprint file before adding a prompt resource.')
   const nodeId=crypto.randomUUID();const snapshot=await prompts.select(fragment.raw.reference,{kind:'blueprint',blueprint_id:graph,node_id:nodeId})
   prompts.pendingBlueprint.push({file,snapshot,nodeId});nodes.value.push(fromSnapshot(snapshot));notice.value='Added to the open blueprint. Return to its canvas to inspect the Prompt resource → ContextMerge connection, then save before running.'
  }
 }catch(reason){notice.value=String(reason)}
}
function drag(event:DragEvent){if(selected.value?.raw)event.dataTransfer?.setData('application/x-metteur-prompt',JSON.stringify(selected.value.raw.reference))}
async function drop(event:DragEvent,target:'chat'|'blueprint'){
 const data=event.dataTransfer?.getData('application/x-metteur-prompt');if(!data)return
 try{const reference=JSON.parse(data);const path=workspace.active?.path;if(!path)return;const result=await gateway.resourceRequest('queryPromptResources',path,{operation:'read',scope:reference.scope.scope,id:reference.id});if(!result.ok)throw new Error(result.error);await insert(target,fromEntry(result.data as PromptEntry))}catch(reason){notice.value=String(reason)}
}
function changed(item:Fragment){return prompts.selections.find(chosen=>chosen.snapshot.id===item.selectionId)?.status==='changed'}
function missing(item:Fragment){return prompts.selections.find(chosen=>chosen.snapshot.id===item.selectionId)?.status==='missing'}
async function refresh(item:Fragment){const index=prompts.selections.findIndex(chosen=>chosen.snapshot.id===item.selectionId);if(index>=0)await prompts.refresh(index)}
async function send(){const chosen=[...chat.value];if(await chatStore.send('Use these selected prompt resources for this request.')){sent.value.push(...chosen);notice.value='Message sent. Conversation history retains immutable selected snapshots.'}else notice.value=prompts.error||'The message was not sent. Open Chat for details.'}
async function save(){
 const item=selected.value,path=workspace.active?.path;if(!item?.raw||!path)return
 const result=await gateway.resourceRequest('writePromptResource',path,{scope:item.raw.reference.scope.scope,draft:{id:item.id,name:item.name,tags:item.raw.content.header.tags,body:draft.value}},item.revision)
 if(workspace.active?.path!==path)return
 if(!result.ok){notice.value=result.error;return}editing.value=false;await load();await prompts.validateChat();notice.value='Saved a new source revision. Pending selections must be reviewed again; sent history is unchanged.'
}
async function copy(){const item=selected.value,path=workspace.active?.path;if(!item?.raw||!path)return;const result=await gateway.resourceRequest('copyPromptToProject',path,{reference:item.raw.reference});if(workspace.active?.path!==path)return;if(!result.ok){notice.value=result.error;return}scope.value='Project';selection.value=(result.data as PromptEntry).reference.id;await load();notice.value='Copied to Project with a new ID and recorded origin. Future edits are independent.'}
async function remove(){const item=selected.value,path=workspace.active?.path;if(!item?.raw||!path)return;const result=await gateway.resourceRequest('deletePromptResource',path,{scope:item.raw.reference.scope.scope,id:item.id},item.revision);if(workspace.active?.path!==path)return;if(!result.ok){notice.value=result.error;return}deleting.value=false;await load();await prompts.validateChat();notice.value='Source removed. Sent snapshots remain in conversation history; pending selections now require attention.'}
async function reviewRun(){const path=workspace.active?.path;if(!path)return;for(const node of nodes.value){const result=await gateway.resourceRequest('queryPromptResources',path,{operation:'read',scope:node.scope.toLowerCase(),id:node.id});if(!result.ok||(result.data as PromptEntry).reference.revision!==node.revision){notice.value='Run resource changed or is missing. Select the current version again before running.';return}}notice.value='Selected run resource versions are current. Save the blueprint and use its Run control; execution freezes this manifest.'}
</script>
<template>
  <section aria-label="Prompt resources">
    <header class="pv-heading"><div><p class="pv-eyebrow">RESOURCE LIBRARY</p><h1>Prompt resources</h1><p class="pv-muted">Choose reusable instructions for this message or blueprint run.</p></div><button class="btn" @click="load">Reload</button></header>
    <div class="pv-toolbar"><button class="btn" :class="{ 'btn-primary': scope === 'Project' }" @click="scope = 'Project'; editing = false">Project</button><button class="btn" :class="{ 'btn-primary': scope === 'Personal' }" @click="scope = 'Personal'; editing = false">Personal</button><span class="pv-muted">{{ scope === 'Personal' ? 'Visible to your authenticated account. Never automatically added to project context.' : 'Available only in the current workspace.' }}</span></div>
    <p v-if="scenario === 'Loading'" role="status" class="pv-callout">Loading resources…</p>
    <p v-else-if="scenario === 'Error'" role="alert" class="pv-callout">Resources could not be loaded. <button class="btn" @click="load">Retry</button></p>
    <template v-else>
      <p v-if="!writable" class="pv-callout">{{ scenario }} · Selection, editing and sending are disabled.</p>
      <div class="pv-split"><aside class="pv-list"><p class="pv-eyebrow">{{ scope.toUpperCase() }} / {{ shown.length }} RESOURCES</p><button v-for="item in shown" :key="item.id" class="pv-row" :class="{ selected: selected?.id === item.id }" @click="selection = item.id; editing = false"><span class="pv-muted">{{ item.scope }} · r{{ item.revision }}</span><strong>{{ item.name }}</strong><span class="pv-muted">{{ item.id }} · .prompt.md</span></button><p v-if="!shown.length" class="pv-empty">No prompt resources in this scope.</p></aside>
        <article v-if="selected" class="pv-detail"><div class="pv-between"><span class="pv-badge">{{ selected.scope }}</span><span class="pv-muted">r{{ selected.revision }} · {{ selected.id }}</span></div><h2 draggable="true" @dragstart="drag">{{ selected.name }} <span class="pv-muted">⋮⋮ Drag</span></h2><label v-if="editing">Prompt body<textarea v-model="draft" class="input pv-editor" /></label><p v-else class="pv-body">{{ selected.body }}</p><p class="pv-muted">Same names in Project and Personal remain separate resources. Selection preserves scope and revision.</p><div class="pv-actions"><button class="btn btn-primary" :disabled="!writable" @click="insert('chat')">Add to Chat</button><button class="btn" :disabled="!writable" @click="insert('blueprint')">Add to blueprint</button><button class="btn" v-if="!editing" :disabled="!writable" @click="draft = selected.body; editing = true">Edit</button><button class="btn" v-if="editing" :disabled="!writable" @click="save">Save revision</button><button class="btn" v-if="editing" @click="editing = false">Cancel</button><button class="btn" v-if="selected.scope === 'Personal'" :disabled="!writable" @click="copy">Copy to Project</button><button class="btn" :disabled="!writable" @click="deleting = true">Delete source</button></div></article><p v-else class="pv-empty">Choose a resource to see its content and revision.</p>
      </div>
      <div class="pv-toolbar"><section class="pv-drop" style="flex:1" aria-label="Chat drop target" @dragover.prevent @drop.prevent="drop($event, 'chat')"><strong>Chat composer</strong><p class="pv-muted">Drop a resource here, or use Add to Chat.</p><div v-for="(item, index) in chat" :key="index" class="pv-chip"><button @click="previewBody = item">{{ item.scope }} / {{ item.name }} · r{{ item.revision }}</button><span class="pv-warn" v-if="missing(item)">Source missing</span><template v-else-if="changed(item)"><span class="pv-warn">Changed — review required</span><button class="pv-link" @click="refresh(item)">Refresh selection</button></template><button :aria-label="`Remove chat prompt ${index + 1}`" @click="prompts.selections.splice(index, 1)">×</button></div><button class="btn btn-primary" :disabled="!writable || !chat.length" @click="send">Send message</button></section>
      <section class="pv-drop" style="flex:1" aria-label="Blueprint drop target" @dragover.prevent @drop.prevent="drop($event, 'blueprint')"><strong>Blueprint canvas</strong><p class="pv-muted">Prompt resource → frozen Text → ContextMerge (user)</p><div v-for="(item, index) in nodes" :key="index" class="pv-chip"><button @click="previewBody = item">{{ item.scope }} / {{ item.name }} · r{{ item.revision }}</button><button :aria-label="`Remove blueprint prompt ${index + 1}`" @click="nodes.splice(index, 1)">×</button></div><button class="btn" :disabled="!writable || !nodes.length" @click="reviewRun">Review run resources</button></section></div>
      <section v-if="sent.length" class="pv-callout"><strong>Sent history · immutable user content</strong><div v-for="(item, index) in sent" :key="index"><p>{{ item.scope }} / {{ item.name }} · r{{ item.revision }}</p><p>{{ item.body }}</p></div><button class="pv-link" @click="notice = 'History clearing uses the conversation deletion control. Removing a source cannot retract content already sent to a model.'">History removal information</button></section>
    </template>
    <p v-if="notice" role="status" class="pv-callout">{{ notice }}</p>
    <div v-if="deleting" v-dialog-focus class="pv-overlay" @keydown.esc="deleting = false"><section role="dialog" aria-modal="true" aria-labelledby="delete-title" class="pv-modal"><h2 id="delete-title">Delete this source?</h2><p>Pending selections will become unavailable. Already sent conversation snapshots and approved run snapshots remain unchanged. This cannot retract content already sent to a model.</p><div class="pv-actions"><button class="btn" autofocus @click="deleting = false">Cancel</button><button class="btn btn-danger-outline" @click="remove">Delete source</button></div></section></div>
    <div v-if="previewBody" v-dialog-focus class="pv-overlay" @keydown.esc="previewBody = null"><section role="dialog" aria-modal="true" aria-label="Selected snapshot" class="pv-modal"><h2>{{ previewBody.scope }} / {{ previewBody.name }}</h2><p>Selected revision {{ previewBody.revision }}</p><p class="pv-body">{{ previewBody.body }}</p><button class="btn" autofocus @click="previewBody = null">Close</button></section></div>
  </section>
</template>
