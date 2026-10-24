<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { gateway } from '@/core'
import { useWorkspaceStore } from '@/stores/workspace'
import { useChatStore } from '@/stores/chat'
import { useSurfaceNavigation } from '@/lib/surface'
import { type SavedResultReference, validResultReference } from '@/core/workflow'
import { useCodingWorkflowsStore } from '@/stores/coding-workflows'
const workflows=useCodingWorkflowsStore()
const props = defineProps<{
  reference: SavedResultReference
  label: string
  sourceLabel: string
  expanded?: boolean
  allowNewPlan?: boolean
}>()
const workspace = useWorkspaceStore(),
  chat = useChatStore(),
  { openSurface } = useSurfaceNavigation()
const saved = ref<{ reference: SavedResultReference; content: string } | null>(null),
  error = ref(''),
  busy = ref(false),
  opened = ref(!!props.expanded),
  notice = ref('')
let generation = 0,
  disposed = false
watch(
  [() => workspace.active?.path, () => JSON.stringify(props.reference)],
  () => {
    generation++
    saved.value = null
    error.value = ''
    busy.value = false
    notice.value = ''
    if (opened.value) void load()
  },
  { immediate: true, flush: 'sync' },
)
onBeforeUnmount(() => {
  disposed = true
  generation++
})
async function load() {
  const root = workspace.active?.path,
    ticket = generation
  if (!root || busy.value) return
  busy.value = true
  try {
    const result = await gateway.resourceRequest('queryContinuations', root, {
      operation: 'capture_result',
      reference: props.reference,
    })
    if (disposed || ticket !== generation || root !== workspace.active?.path) return
    const data = result.ok
      ? (result.data as { reference: SavedResultReference; content: string })
      : null
    if (
      !result.ok ||
      !data ||
      !validResultReference(data.reference) ||
      typeof data.content !== 'string'
    ) {
      error.value = result.ok ? 'The host did not confirm this result.' : result.error
      return
    }
    saved.value = JSON.parse(JSON.stringify(data))
    error.value = ''
  } catch (cause) {
    if (ticket === generation) error.value = String(cause)
  } finally {
    if (ticket === generation) busy.value = false
  }
}
function toggle(event: Event) {
  opened.value = (event.currentTarget as HTMLDetailsElement).open
  if (opened.value && !saved.value) void load()
}
const body = computed(() => {
  try {
    return saved.value ? JSON.parse(saved.value.content) : null
  } catch {
    return null
  }
})
const run = computed(() => body.value?.run ?? body.value)
const reviewRun=computed(()=>props.reference.kind==='run'?props.reference.runId:run.value?.run_id??body.value?.result?.run_id)
function reviewDelivery(){if(workspace.active&&typeof reviewRun.value==='string'){void workflows.openReview(workspace.active.path,reviewRun.value);openSurface('tasks')}}
const checks = computed(() =>
  Array.isArray(body.value?.recorded_checks)
    ? body.value.recorded_checks.filter(
        (c: { current?: boolean; check?: boolean }) =>
          c.current !== false && typeof c.check === 'boolean',
      )
    : [],
)
const checkSummary = computed(() => {
  const count = body.value?.checks_summary?.recorded ?? checks.value.length
  const failed =
    body.value?.checks_summary?.failed ??
    checks.value.filter((c: { check?: boolean }) => c.check === false).length
  return !count
    ? 'No deterministic check recorded'
    : failed
      ? `${failed} check(s) failed`
      : `${count} recorded check(s) passed`
})
const destination = computed(
  () => chat.threads.find((t) => t.sessionId === chat.sessionId)?.title || 'New conversation',
)
function add(newPlan = false) {
  if (!saved.value || !workspace.active?.path || chat.sessionUnavailable) return
  chat.queueResult({ label: props.label, reference: saved.value.reference })
  const instruction = newPlan
    ? 'Prepare a new proposal from the selected saved result. Recheck current versions, scope, permissions and limits; preserve the prior task history and root budget. Do not restart an old run.'
    : 'Review the selected saved result.'
  chat.draft = [chat.draft, instruction].filter(Boolean).join('\n\n')
  notice.value = 'Added to the current draft. Send when ready.'
  openSurface('chat')
}
</script>
<template>
  <details class="workflow-result" :open="expanded" @toggle="toggle">
    <summary>{{ label }} · saved result</summary>
    <p>{{ sourceLabel }}</p>
    <p v-if="busy" role="status">Reading the saved result…</p>
    <p v-if="error" role="alert">
      {{ error }} <button class="btn" @click="load">Retry reading</button>
    </p>
    <template v-if="saved">
      <dl>
        <dt>Execution</dt>
        <dd>{{ run?.status ?? 'Recorded source result' }}</dd>
        <dt>Checks</dt>
        <dd>{{ checkSummary }}</dd>
        <dt>Human acceptance</dt>
        <dd>{{ reviewRun ? 'Open the delivery review for current acceptance' : 'Not recorded for this result type' }}</dd>
      </dl>
      <p v-if="run?.error" role="alert">{{ run.error }}</p>
      <p v-if="run?.in_flight || run?.unresolved_background_effects" role="alert">
        Effects remain unconfirmed. Inspect the recorded files and receipts.
      </p>
      <details>
        <summary>Checks, outputs &amp; source details</summary>
        <p>
          Recorded outputs may contain model opinions. They do not replace deterministic checks or
          human acceptance.
        </p>
        <pre>{{ saved.content }}</pre>
        <p>{{ JSON.stringify(saved.reference) }}</p>
      </details>
      <p class="destination">Current draft: {{ destination }}</p>
      <div class="actions">
        <button v-if="reviewRun" class="btn" @click="reviewDelivery">Review delivery</button>
        <button class="btn" :disabled="chat.sessionUnavailable" @click="add()">
          Add result to draft</button
        ><button
          v-if="allowNewPlan"
          class="btn"
          :disabled="chat.sessionUnavailable"
          @click="add(true)"
        >
          Prepare new proposal
        </button>
      </div>
      <p v-if="notice" role="status">{{ notice }}</p>
    </template>
  </details>
</template>
<style scoped>
.workflow-result {
  font-size: 13px;
  line-height: 1.65;
  border: 1px solid var(--divider);
  border-radius: 8px;
  padding: 12px;
  margin: 10px 0;
  overflow-wrap: anywhere;
}
summary {
  cursor: pointer;
  font-weight: 550;
}
p,
dl {
  margin: 9px 0;
}
dl {
  display: grid;
  grid-template-columns: 110px minmax(0, 1fr);
  gap: 5px 12px;
}
dt,
.destination {
  color: var(--muted-foreground);
}
.actions {
  display: flex;
  gap: 10px;
  flex-wrap: wrap;
}
pre {
  max-height: 320px;
  overflow: auto;
  white-space: pre-wrap;
  font-size: 13px;
  padding: 10px;
}
[role='alert'] {
  color: var(--danger);
}
</style>
