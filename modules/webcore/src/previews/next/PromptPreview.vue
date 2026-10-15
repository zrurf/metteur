<script setup lang="ts">
import { computed, ref } from 'vue'
import { vDialogFocus } from './dialog-focus'
const props = defineProps<{ scenario: string }>()
type Fragment = { id: string; scope: string; name: string; revision: number; body: string }
const scope = ref('Project')
const resources = ref<Fragment[]>([
  { id: 'project-1', scope: 'Project', name: 'Review checklist', revision: 3, body: 'Review changed behavior, permission boundaries and recovery paths. Cite the relevant source and report checks that were not run.' },
  { id: 'personal-1', scope: 'Personal', name: 'Review checklist', revision: 7, body: 'Explain findings in priority order. Use concise language and include a concrete reproduction where possible.' },
  { id: 'project-2', scope: 'Project', name: 'Comment style', revision: 1, body: 'Write concise English comments. Explain reasoning rather than restating the code.' },
])
const selection = ref('project-1')
const shown = computed(() => props.scenario === 'Empty' ? [] : resources.value.filter(r => r.scope === scope.value))
const selected = computed(() => shown.value.find(r => r.id === selection.value) ?? shown.value[0])
const chat = ref<Fragment[]>([])
const nodes = ref<Fragment[]>([])
const sent = ref<Fragment[]>([])
const notice = ref('')
const editing = ref(false)
const draft = ref('')
const deleting = ref(false)
const previewBody = ref<Fragment | null>(null)
const writable = computed(() => !['Read only', 'Permission denied', 'Disconnected'].includes(props.scenario))
function insert(target: 'chat' | 'blueprint', fragment = selected.value) {
  if (!fragment || !writable.value) return
  ;(target === 'chat' ? chat : nodes).value.push({ ...fragment })
  notice.value = target === 'chat' ? 'Selected as user-level content. Repeated selections are kept in order.' : 'Prompt resource node added. Its frozen Text output connects to ContextMerge as user content.'
}
function drag(event: DragEvent) { if (selected.value) event.dataTransfer?.setData('application/x-metteur-preview-prompt', selected.value.id) }
function drop(event: DragEvent, target: 'chat' | 'blueprint') {
  const id = event.dataTransfer?.getData('application/x-metteur-preview-prompt')
  const item = resources.value.find(r => r.id === id)
  if (item) insert(target, item)
}
function changed(item: Fragment) { return props.scenario === 'Changed version' || resources.value.find(r => r.id === item.id)?.revision !== item.revision }
function missing(item: Fragment) { return props.scenario === 'Missing source' || !resources.value.some(r => r.id === item.id) }
function refresh(item: Fragment) {
  const source = resources.value.find(r => r.id === item.id)
  if (!source) return
  Object.assign(item, source); notice.value = `Selected current revision ${source.revision}. The Changed version fixture stays active until you choose Ready.`
}
function send() {
  if (chat.value.some(item => missing(item) || changed(item))) { notice.value = 'Source changed or is missing. Review and refresh your selection before sending.'; return }
  sent.value.push(...chat.value.map(item => ({ ...item }))); chat.value = []; notice.value = 'Fixture message sent. History contains immutable user-selected snapshots.'
}
function save() {
  if (!selected.value) return
  if (props.scenario === 'Conflict') { notice.value = 'The file changed while you were editing. Your draft is preserved; reload before saving.'; return }
  selected.value.body = draft.value; selected.value.revision++; editing.value = false; notice.value = 'Saved a new source revision. Pending selections must be reviewed again; sent history is unchanged.'
}
function copy() {
  if (!selected.value) return
  const item = { ...selected.value, id: `copy-${resources.value.length}`, scope: 'Project', revision: 1 }
  resources.value.push(item); scope.value = 'Project'; selection.value = item.id
  notice.value = 'Copied to Project with a new ID and recorded origin. Future edits are independent.'
}
function remove() {
  resources.value = resources.value.filter(r => r.id !== selected.value?.id); deleting.value = false
  notice.value = 'Source removed. Sent snapshots remain in conversation history; pending selections now require attention.'
}
</script>
<template>
  <section aria-label="Prompt resources preview">
    <header class="pv-heading"><div><p class="pv-eyebrow">RESOURCE LIBRARY</p><h1>Prompt resources</h1><p class="pv-muted">Choose reusable instructions for this message or blueprint run.</p></div><span class="pv-badge">PV-02 · r1</span></header>
    <div class="pv-toolbar"><button class="btn" :class="{ 'btn-primary': scope === 'Project' }" @click="scope = 'Project'; editing = false">Project</button><button class="btn" :class="{ 'btn-primary': scope === 'Personal' }" @click="scope = 'Personal'; editing = false">Personal</button><span class="pv-muted">{{ scope === 'Personal' ? 'Visible to your authenticated account. Never automatically added to project context.' : 'Available only in the current workspace.' }}</span></div>
    <p v-if="scenario === 'Loading'" role="status" class="pv-callout">Loading resources…</p>
    <p v-else-if="scenario === 'Error'" role="alert" class="pv-callout">Resources could not be loaded. <button class="btn" @click="notice = 'Fixture retry requested.'">Retry</button></p>
    <template v-else>
      <p v-if="!writable" class="pv-callout">{{ scenario }} · Selection, editing and sending are disabled.</p>
      <div class="pv-split"><aside class="pv-list"><p class="pv-eyebrow">{{ scope.toUpperCase() }} / {{ shown.length }} RESOURCES</p><button v-for="item in shown" :key="item.id" class="pv-row" :class="{ selected: selected?.id === item.id }" @click="selection = item.id; editing = false"><span class="pv-muted">{{ item.scope }} · r{{ item.revision }}</span><strong>{{ item.name }}</strong><span class="pv-muted">{{ item.id }} · .prompt.md</span></button><p v-if="!shown.length" class="pv-empty">No prompt resources in this scope.</p></aside>
        <article v-if="selected" class="pv-detail"><div class="pv-between"><span class="pv-badge">{{ selected.scope }}</span><span class="pv-muted">r{{ selected.revision }} · {{ selected.id }}</span></div><h2 draggable="true" @dragstart="drag">{{ selected.name }} <span class="pv-muted">⋮⋮ Drag</span></h2><label v-if="editing">Prompt body<textarea v-model="draft" class="input pv-editor" /></label><p v-else class="pv-body">{{ selected.body }}</p><p class="pv-muted">Same names in Project and Personal remain separate resources. Selection preserves scope and revision.</p><div class="pv-actions"><button class="btn btn-primary" :disabled="!writable" @click="insert('chat')">Add to Chat</button><button class="btn" :disabled="!writable" @click="insert('blueprint')">Add to blueprint</button><button class="btn" v-if="!editing" :disabled="!writable" @click="draft = selected.body; editing = true">Edit</button><button class="btn" v-if="editing" :disabled="!writable" @click="save">Save revision</button><button class="btn" v-if="editing" @click="editing = false">Cancel</button><button class="btn" v-if="selected.scope === 'Personal'" :disabled="!writable" @click="copy">Copy to Project</button><button class="btn" :disabled="!writable" @click="deleting = true">Delete source</button></div></article><p v-else class="pv-empty">Choose a resource to see its content and revision.</p>
      </div>
      <div class="pv-toolbar"><section class="pv-drop" style="flex:1" aria-label="Chat drop target" @dragover.prevent @drop.prevent="drop($event, 'chat')"><strong>Chat composer</strong><p class="pv-muted">Drop a resource here, or use Add to Chat.</p><div v-for="(item, index) in chat" :key="index" class="pv-chip"><button @click="previewBody = item">{{ item.scope }} / {{ item.name }} · r{{ item.revision }}</button><span class="pv-warn" v-if="missing(item)">Source missing</span><template v-else-if="changed(item)"><span class="pv-warn">Changed — review required</span><button class="pv-link" @click="refresh(item)">Refresh selection</button></template><button :aria-label="`Remove chat prompt ${index + 1}`" @click="chat.splice(index, 1)">×</button></div><button class="btn btn-primary" :disabled="!writable || !chat.length" @click="send">Send fixture message</button></section>
      <section class="pv-drop" style="flex:1" aria-label="Blueprint drop target" @dragover.prevent @drop.prevent="drop($event, 'blueprint')"><strong>Blueprint canvas</strong><p class="pv-muted">Prompt resource → frozen Text → ContextMerge (user)</p><div v-for="(item, index) in nodes" :key="index" class="pv-chip"><button @click="previewBody = item">{{ item.scope }} / {{ item.name }} · r{{ item.revision }}</button><button :aria-label="`Remove blueprint prompt ${index + 1}`" @click="nodes.splice(index, 1)">×</button></div><button class="btn" :disabled="!writable || !nodes.length" @click="notice = nodes.some(item => missing(item) || changed(item)) ? 'Run manifest needs review: a source is missing or changed.' : 'Fixture manifest frozen. Later edits cannot change this run. No blueprint was executed.'">Review run resources</button></section></div>
      <section v-if="sent.length" class="pv-callout"><strong>Sent history · immutable user content</strong><div v-for="(item, index) in sent" :key="index"><p>{{ item.scope }} / {{ item.name }} · r{{ item.revision }}</p><p>{{ item.body }}</p></div><button class="pv-link" @click="notice = 'History clearing uses the conversation deletion control. Removing a source cannot retract content already sent to a model.'">History removal information</button></section>
    </template>
    <p v-if="notice" role="status" class="pv-callout">{{ notice }}</p>
    <div v-if="deleting" v-dialog-focus class="pv-overlay" @keydown.esc="deleting = false"><section role="dialog" aria-modal="true" aria-labelledby="delete-title" class="pv-modal"><h2 id="delete-title">Delete this source?</h2><p>Pending selections will become unavailable. Already sent conversation snapshots and approved run snapshots remain unchanged. This cannot retract content already sent to a model.</p><div class="pv-actions"><button class="btn" autofocus @click="deleting = false">Cancel</button><button class="btn btn-danger-outline" @click="remove">Delete source</button></div></section></div>
    <div v-if="previewBody" v-dialog-focus class="pv-overlay" @keydown.esc="previewBody = null"><section role="dialog" aria-modal="true" aria-label="Selected snapshot" class="pv-modal"><h2>{{ previewBody.scope }} / {{ previewBody.name }}</h2><p>Selected revision {{ previewBody.revision }}</p><p class="pv-body">{{ previewBody.body }}</p><button class="btn" autofocus @click="previewBody = null">Close</button></section></div>
  </section>
</template>
