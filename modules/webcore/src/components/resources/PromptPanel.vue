<script setup lang="ts">
import MoreActions from './MoreActions.vue'
import PageHeader from '@/components/PageHeader.vue'
import PageTabs from '@/components/PageTabs.vue'
import InlineHelp from '@/components/resources/InlineHelp.vue'
import { computed, ref, watch } from 'vue'
import { vDialogFocus } from './dialog-focus'
import { gateway } from '@/core'
import { useWorkspaceStore } from '@/stores/workspace'
import { usePromptResourcesStore } from '@/stores/prompt-resources'
import { useBlueprintStore } from '@/stores/blueprint'
import { useSurfaceNavigation } from '@/lib/surface'
import type { PromptEntry, PromptSnapshot } from '@/core/resources'
import './resources.css'
import { useRouter } from 'vue-router'
import { useTabsStore } from '@/stores/tabs'
import { promptFilePath } from '@/lib/prompt-file'
import { fileRoute } from '@/lib/file-token'
import { useDocumentsStore } from '@/stores/documents'
const router = useRouter(),
  tabs = useTabsStore()
const workspace = useWorkspaceStore(),
  prompts = usePromptResourcesStore(),
  blueprint = useBlueprintStore(),
  { openSurface } = useSurfaceNavigation()
type Fragment = {
  id: string
  scope: string
  name: string
  revision: number
  body: string
  raw?: PromptEntry
  selectionId?: string
}
const scope = ref('Project'),
  selection = ref(''),
  resources = ref<Fragment[]>([]),
  notice = ref(''),
  deleting = ref(false),
  previewBody = ref<Fragment | null>(null),
  busy = ref(false),
  failed = ref(false)
const nodes = ref<Fragment[]>([]),
  search = ref(''),
  next = ref<string | null>(null)
let generation = 0
const shown = computed(() =>
  resources.value.filter(
    (item) =>
      item.scope === scope.value &&
      `${item.name} ${item.body}`.toLowerCase().includes(search.value.toLowerCase()),
  ),
)
const selected = computed(
  () => shown.value.find((item) => item.id === selection.value) ?? shown.value[0],
)
const chat = computed(() => prompts.selections.map((item) => fromSnapshot(item.snapshot)))
const scenario = computed(() =>
  !gateway.connected.value
    ? 'Disconnected'
    : busy.value
      ? 'Loading'
      : failed.value
        ? 'Error'
        : 'Ready',
)
const writable = computed(() => gateway.connected.value && !busy.value && !failed.value)
function fromEntry(entry: PromptEntry): Fragment {
  return {
    id: entry.reference.id,
    scope: entry.reference.scope.scope === 'project' ? 'Project' : 'Personal',
    name: entry.content.header.name,
    revision: entry.reference.revision,
    body: entry.content.body,
    raw: entry,
  }
}
function fromSnapshot(snapshot: PromptSnapshot): Fragment {
  return {
    id: snapshot.reference.id,
    scope: snapshot.reference.scope.scope === 'project' ? 'Project' : 'Personal',
    name: snapshot.name,
    revision: snapshot.reference.revision,
    body: snapshot.body,
    selectionId: snapshot.id,
  }
}
async function load(append = false) {
  const path = workspace.active?.path
  if (!path) return
  const ticket = ++generation
  busy.value = true
  failed.value = false
  const result = await gateway.resourceRequest('queryPromptResources', path, {
    operation: 'list',
    scope: scope.value.toLowerCase(),
    limit: 50,
    after: append ? next.value : null,
  })
  if (ticket !== generation || workspace.active?.path !== path) return
  busy.value = false
  failed.value = !result.ok
  if (!result.ok) {
    notice.value = result.error
    return
  }
  const page = (result.data as PromptEntry[]).map(fromEntry)
  resources.value = append
    ? [...resources.value, ...page.filter((p) => !resources.value.some((r) => r.id === p.id))]
    : page
  next.value = page.length === 50 ? page[page.length - 1]!.id : null
}
watch(
  [() => workspace.active?.path, scope],
  ([path], [oldPath]) => {
    if (path !== oldPath) {
      nodes.value = []
      notice.value = ''
    }
    resources.value = []
    deleting.value = false
    previewBody.value = null
    void load()
  },
  { immediate: true, flush: 'sync' },
)
async function insert(target: 'chat' | 'blueprint', fragment = selected.value) {
  if (!fragment?.raw || !writable.value) return
  try {
    if (target === 'chat') {
      await prompts.addChat(fragment.raw.reference)
      notice.value = 'Added to Chat.'
      openSurface('chat')
    } else {
      const file = blueprint.currentFile,
        graph = blueprint.uuidFor(file)
      if (!file || !graph) throw new Error('Open a blueprint file before adding a prompt resource.')
      const nodeId = crypto.randomUUID()
      const snapshot = await prompts.select(fragment.raw.reference, {
        kind: 'blueprint',
        blueprint_id: graph,
        node_id: nodeId,
      })
      prompts.pendingBlueprint.push({ file, snapshot, nodeId })
      nodes.value.push(fromSnapshot(snapshot))
      notice.value =
        'Added to the open blueprint. Return to its canvas to inspect the Prompt resource → ContextMerge connection, then save before running.'
    }
  } catch (reason) {
    notice.value = String(reason)
  }
}
function drag(event: DragEvent) {
  if (selected.value?.raw)
    event.dataTransfer?.setData(
      'application/x-metteur-prompt',
      JSON.stringify(selected.value.raw.reference),
    )
}
async function drop(event: DragEvent, target: 'chat' | 'blueprint') {
  const data = event.dataTransfer?.getData('application/x-metteur-prompt')
  if (!data) return
  try {
    const reference = JSON.parse(data)
    const path = workspace.active?.path
    if (!path) return
    const result = await gateway.resourceRequest('queryPromptResources', path, {
      operation: 'read',
      scope: reference.scope.scope,
      id: reference.id,
    })
    if (!result.ok) throw new Error(result.error)
    await insert(target, fromEntry(result.data as PromptEntry))
  } catch (reason) {
    notice.value = String(reason)
  }
}
function changed(item: Fragment) {
  return (
    prompts.selections.find((chosen) => chosen.snapshot.id === item.selectionId)?.status ===
    'changed'
  )
}
function missing(item: Fragment) {
  return (
    prompts.selections.find((chosen) => chosen.snapshot.id === item.selectionId)?.status ===
    'missing'
  )
}
async function refresh(item: Fragment) {
  const index = prompts.selections.findIndex((chosen) => chosen.snapshot.id === item.selectionId)
  if (index >= 0) await prompts.refresh(index)
}
function editSelected() {
  const item = selected.value
  if (!item?.raw || !writable.value) return
  const path = promptFilePath(item.raw)
  tabs.openFile(path, `${item.name}.md`)
  void router.push(fileRoute(path))
}
async function copy() {
  const item = selected.value,
    path = workspace.active?.path
  if (!item?.raw || !path) return
  const result = await gateway.resourceRequest('copyPromptToProject', path, {
    reference: item.raw.reference,
  })
  if (workspace.active?.path !== path) return
  if (!result.ok) {
    notice.value = result.error
    return
  }
  scope.value = 'Project'
  selection.value = (result.data as PromptEntry).reference.id
  await load()
  notice.value =
    'Copied to Project with a new ID and recorded origin. Future edits are independent.'
}
async function remove() {
  const item = selected.value,
    path = workspace.active?.path
  if (!item?.raw || !path) return
  const result = await gateway.resourceRequest(
    'deletePromptResource',
    path,
    { scope: item.raw.reference.scope.scope, id: item.id },
    item.revision,
  )
  if (workspace.active?.path !== path) return
  if (!result.ok) {
    notice.value = result.error
    return
  }
  deleting.value = false
  const documents = useDocumentsStore(),
    file = promptFilePath(item.raw),
    document = documents.peek(path, file)
  if (document && result.data)
    document.resource = document.diskResource = result.data as PromptEntry
  documents.markDeleted(path, file)
  await load()
  await prompts.validateChat()
  notice.value =
    'Source removed. Sent snapshots remain in conversation history; pending selections now require attention.'
}
async function reviewRun() {
  const path = workspace.active?.path
  if (!path) return
  for (const node of nodes.value) {
    const result = await gateway.resourceRequest('queryPromptResources', path, {
      operation: 'read',
      scope: node.scope.toLowerCase(),
      id: node.id,
    })
    if (!result.ok || (result.data as PromptEntry).reference.revision !== node.revision) {
      notice.value =
        'Run resource changed or is missing. Select the current version again before running.'
      return
    }
  }
  notice.value =
    'Selected run resource versions are current. Save the blueprint and use its Run control; execution freezes this manifest.'
}
</script>
<template>
  <section aria-label="Prompt Library">
    <PageHeader title="Prompt Library"
      ><template #help
        ><InlineHelp
          text="Reuse versioned instructions in Chat or an open blueprint. Project and Personal resources remain separate." /></template
      ><button class="btn btn-outline" @click="load()">Reload</button></PageHeader
    >
    <PageTabs
      :model-value="scope"
      label="Prompt Scope"
      :items="[
        { value: 'Project', label: 'Project' },
        { value: 'Personal', label: 'Personal' },
      ]"
      @update:model-value="scope = $event as 'Project' | 'Personal'"
    />
    <div class="pv-toolbar">
      <input
        v-model="search"
        class="input pv-search"
        aria-label="Search loaded prompt resources"
        placeholder="Search resources"
      /><InlineHelp text="Search the loaded resources in this scope." />
    </div>
    <p v-if="scenario === 'Loading'" role="status" class="pv-callout">Loading resources…</p>
    <p v-else-if="scenario === 'Error'" role="alert" class="pv-callout">
      Resources could not be loaded. <button class="btn btn-outline" @click="load()">Retry</button>
    </p>
    <template v-else>
      <p v-if="!writable" class="pv-callout">
        {{ scenario }} · Selection, editing and sending are disabled.
      </p>
      <div class="pv-split">
        <aside class="pv-list">
          <button
            v-for="item in shown"
            :key="item.id"
            class="pv-row"
            :class="{ selected: selected?.id === item.id }"
            @click="selection = item.id"
          >
            <span class="pv-muted">{{ item.scope }} · r{{ item.revision }}</span
            ><strong>{{ item.name }}</strong>
          </button>
          <p v-if="!shown.length" class="pv-empty">No matching resources.</p>
          <button v-if="next" class="btn btn-outline" :disabled="busy" @click="load(true)">
            Load more
          </button>
        </aside>
        <article v-if="selected" class="pv-detail">
          <div class="pv-between">
            <span class="pv-badge">{{ selected.scope }}</span
            ><span class="pv-muted">Revision {{ selected.revision }}</span>
          </div>
          <h2
            draggable="true"
            title="Drag this resource into Chat or a blueprint"
            @dragstart="drag"
          >
            {{ selected.name }}
          </h2>
          <p class="pv-body">{{ selected.body }}</p>
          <details class="pv-disclosure">
            <summary>Source details</summary>
            <dl class="pv-facts">
              <dt>Resource ID</dt>
              <dd>{{ selected.id }}</dd>
              <dt>Scope</dt>
              <dd>{{ selected.scope }}</dd>
              <dt>Revision</dt>
              <dd>{{ selected.revision }}</dd>
            </dl>
          </details>
          <div class="pv-actions">
            <button class="btn btn-primary" :disabled="!writable" @click="insert('chat')">
              Use in Chat</button
            ><button class="btn btn-outline" :disabled="!writable" @click="insert('blueprint')">
              Add to blueprint</button
            ><MoreActions
              ><button :disabled="!writable" @click="editSelected">Open in Editor</button
              ><button v-if="selected.scope === 'Personal'" :disabled="!writable" @click="copy">
                Copy to Project</button
              ><button :disabled="!writable" @click="deleting = true">
                Delete source
              </button></MoreActions
            >
          </div>
        </article>
        <p v-else class="pv-empty">Choose a resource to see its content and revision.</p>
      </div>
      <div v-if="chat.length || nodes.length" class="pv-toolbar">
        <section
          v-if="chat.length"
          class="pv-drop"
          style="flex: 1"
          aria-label="Chat drop target"
          @dragover.prevent
          @drop.prevent="drop($event, 'chat')"
        >
          <strong>Chat composer</strong>
          <div v-for="(item, index) in chat" :key="index" class="pv-chip">
            <button @click="previewBody = item">
              {{ item.scope }} / {{ item.name }} · r{{ item.revision }}</button
            ><span class="pv-warn" v-if="missing(item)">Source missing</span
            ><template v-else-if="changed(item)"
              ><span class="pv-warn">Changed — review required</span
              ><button class="pv-link" @click="refresh(item)">Refresh selection</button></template
            ><button
              :aria-label="`Remove chat prompt ${index + 1}`"
              @click="prompts.selections.splice(index, 1)"
            >
              ×
            </button>
          </div>
          <button class="btn btn-outline" @click="openSurface('chat')">Open Chat</button>
        </section>
        <section
          v-if="nodes.length"
          class="pv-drop"
          style="flex: 1"
          aria-label="Blueprint drop target"
          @dragover.prevent
          @drop.prevent="drop($event, 'blueprint')"
        >
          <strong>Blueprint canvas</strong>
          <div v-for="(item, index) in nodes" :key="index" class="pv-chip">
            <button @click="previewBody = item">
              {{ item.scope }} / {{ item.name }} · r{{ item.revision }}</button
            ><button
              :aria-label="`Remove blueprint prompt ${index + 1}`"
              @click="nodes.splice(index, 1)"
            >
              ×
            </button>
          </div>
          <button class="btn btn-outline" :disabled="!writable || !nodes.length" @click="reviewRun">
            Review run resources
          </button>
        </section>
      </div>
    </template>
    <p v-if="notice" role="status" class="pv-callout">{{ notice }}</p>
    <div v-if="deleting" v-dialog-focus class="pv-overlay" @keydown.esc="deleting = false">
      <section role="dialog" aria-modal="true" aria-labelledby="delete-title" class="pv-modal">
        <h2 id="delete-title">Delete This Source?</h2>
        <p>
          Pending selections will become unavailable. Already sent conversation snapshots and
          approved run snapshots remain unchanged. This cannot retract content already sent to a
          model.
        </p>
        <div class="pv-actions">
          <button class="btn btn-outline" autofocus @click="deleting = false">Cancel</button
          ><button class="btn btn-danger-outline" @click="remove">Delete source</button>
        </div>
      </section>
    </div>
    <div v-if="previewBody" v-dialog-focus class="pv-overlay" @keydown.esc="previewBody = null">
      <section role="dialog" aria-modal="true" aria-label="Selected snapshot" class="pv-modal">
        <h2>{{ previewBody.scope }} / {{ previewBody.name }}</h2>
        <p>Selected revision {{ previewBody.revision }}</p>
        <p class="pv-body">{{ previewBody.body }}</p>
        <button class="btn btn-outline" autofocus @click="previewBody = null">Close</button>
      </section>
    </div>
  </section>
</template>
