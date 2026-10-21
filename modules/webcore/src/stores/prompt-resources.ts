import { defineStore } from 'pinia'
import { ref, watch } from 'vue'
import { gateway } from '@/core'
import { useWorkspaceStore } from './workspace'
import type { PromptEntry, PromptSnapshot, ResourceRef } from '@/core/resources'
export const usePromptResourcesStore = defineStore('prompt-resources', () => {
  const workspace=useWorkspaceStore()
  const selections=ref<{snapshot:PromptSnapshot;status:'current'|'changed'|'missing'}[]>([])
  const pendingBlueprint=ref<{file:string;snapshot:PromptSnapshot;nodeId:string}[]>([])
  const error=ref('')
  let workspaceEpoch = 0
  watch(()=>workspace.active?.path,()=>{workspaceEpoch++;selections.value=[];pendingBlueprint.value=[];error.value=''}, { flush:'sync' })
  async function select(reference:ResourceRef,purpose:PromptSnapshot['purpose']) {
    const path=workspace.active?.path, epoch=workspaceEpoch;if(!path) throw new Error('Open a workspace first')
    const result=await gateway.resourceRequest('selectPromptResource',path,{reference,purpose})
    if(epoch!==workspaceEpoch||workspace.active?.path!==path) throw new Error('Workspace changed')
    if(!result.ok) throw new Error(result.error)
    return result.data as PromptSnapshot
  }
  async function addChat(reference:ResourceRef) { const snapshot=await select(reference,{kind:'chat'});selections.value.push({snapshot,status:'current'});return snapshot }
  async function refresh(index:number) {
    const item=selections.value[index];const path=workspace.active?.path,epoch=workspaceEpoch;if(!item||!path)return
    const result=await gateway.resourceRequest('queryPromptResources',path,{operation:'read',scope:item.snapshot.reference.scope.scope,id:item.snapshot.reference.id})
    if(epoch!==workspaceEpoch||workspace.active?.path!==path||selections.value[index]!==item)return
    if(!result.ok){item.status='missing';error.value=result.error;return}
    try {const snapshot=await select((result.data as PromptEntry).reference,{kind:'chat'});if(selections.value[index]===item)selections.value[index]={snapshot,status:'current'}} catch(reason) {if(epoch===workspaceEpoch&&workspace.active?.path===path)error.value=String(reason)}
  }
  async function validateChat() {
    const path=workspace.active?.path,epoch=workspaceEpoch;if(!path)return false
    const chosen=[...selections.value]
    const selectionIsCurrent=()=>epoch===workspaceEpoch&&workspace.active?.path===path&&chosen.length===selections.value.length&&chosen.every((item,index)=>selections.value[index]===item)
    for(const item of chosen){
      const result=await gateway.resourceRequest('queryPromptResources',path,{operation:'read',scope:item.snapshot.reference.scope.scope,id:item.snapshot.reference.id})
      if(!selectionIsCurrent()) {
        if(epoch===workspaceEpoch&&workspace.active?.path===path)error.value='Prompt selections changed. Review them and send again.'
        return false
      }
      const current=result.ok?(result.data as PromptEntry).reference:null
      item.status=!current?'missing':current.revision!==item.snapshot.reference.revision||current.content_digest!==item.snapshot.reference.content_digest?'changed':'current'
    }
    const valid=selectionIsCurrent()&&chosen.every(item=>item.status==='current')
    error.value=valid?'':'A selected prompt changed or is missing. Review and refresh the selection before sending.'
    return valid
  }
  async function dropChat(event:DragEvent) {
    const raw=event.dataTransfer?.getData('application/x-metteur-prompt');if(!raw)return
    event.preventDefault()
    try{await addChat(JSON.parse(raw) as ResourceRef)}catch(reason){error.value=String(reason)}
  }
  return {selections,pendingBlueprint,error,select,addChat,refresh,validateChat,dropChat}
})
