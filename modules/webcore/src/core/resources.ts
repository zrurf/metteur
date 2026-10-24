export type ResourceMethod = 'queryCodingWorkflows' | 'recordWorkflowReview' | 'approvalDelivery' | 'conciergeDelivery' | 'chatDelivery' | 'recovery' | 'addonPages' | 'confirmBlueprintLaunch' | 'queryBlueprintLaunch' | 'queryContinuations' | 'controlContinuation' | 'writeSchedule' | 'queryProjectMemory' | 'writeProjectNote' | 'promoteProjectConvention' | 'updateProjectMemoryStatus' | 'queryPromptResources' | 'writePromptResource' | 'deletePromptResource' | 'copyPromptToProject' | 'selectPromptResource'
export interface ResourceRef { scope: { scope: 'project' | 'personal'; owner: string }; id: string; revision: number; content_digest: string }
export interface MemoryEntry {
  reference: ResourceRef
  content: { title: string; body: string; origin: string; validity: string; author: string; source: string | null; files: { path: string; digest: string }[]; confirmation: unknown }
}
export interface MemoryPage { entries: MemoryEntry[]; next: string | null; truncated: boolean }
export interface PromptEntry { reference:ResourceRef; content:{header:{id:string;name:string;tags:string[]};body:string;raw_digest:string;body_digest:string;deleted:boolean;copied_from:ResourceRef|null} }
export interface PromptSnapshot { id:string; reference:ResourceRef; name:string; body:string; selected_by:string; selected_at:number; purpose:{kind:'chat'|'blueprint';blueprint_id?:string;node_id?:string} }
