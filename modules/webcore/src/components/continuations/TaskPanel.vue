<script setup lang="ts">
import InlineHelp from '@/components/resources/InlineHelp.vue'
import PageHeader from '@/components/PageHeader.vue'
import { computed, onUnmounted, ref, watch } from 'vue'
import { useExecutionStore } from '@/stores/execution'
import TaskStartDialog from './TaskStartDialog.vue'
import { useConfigStore } from '@/stores/config'
import { Pause, Play, Square, Terminal, ArrowUpRight, RefreshCw, Plus } from '@lucide/vue'
import { gateway } from '@/core'
import { useWorkspaceStore } from '@/stores/workspace'
import { useChatStore } from '@/stores/chat'
import { useSurfaceNavigation } from '@/lib/surface'
import { vDialogFocus } from '@/components/resources/dialog-focus'
import type { TaskRecord, TaskPage } from '@/core/continuations'
import { stateLabel } from '@/core/continuations'
import './continuations.css'
import { useWorkspaceRefresh } from '@/lib/workspace-refresh'
import WorkflowResult from '@/components/WorkflowResult.vue'
import { useComposerInputsStore } from '@/stores/composer-inputs'
defineProps<{ embedded?: boolean }>()
const chat = useChatStore(),
  workspace = useWorkspaceStore(),
  { openSurface } = useSurfaceNavigation(),
  rows = ref<TaskRecord[]>([]),
  selected = ref(''),
  busy = ref(false),
  loading = ref(true),
  error = ref(''),
  confirm = ref(false),
  next = ref<string | null>(null)
const entry = computed(() => rows.value.find((r) => r.task.id === selected.value) ?? rows.value[0])
const facts = computed(() => entry.value?.workflow)
const allowed = (action: string) => !!facts.value?.allowed_actions.includes(action)
const stopTarget = ref<{ root: string; id: string; revision: number; goal: string } | null>(null)
const taskPrepared = ref<{ root: string; entity: string } | null>(null)
interface RebindProposal {
  task: string
  revision: number
  session: string
  session_revision: number
  digest: string
  goal: string
  latest_user_input: string | null
  policy: { expires_ms: number; detached: boolean }
  budget: TaskRecord['task']['budget']
  wait: NonNullable<TaskRecord['task']['wait']>
}
const rebindReview = ref<{ root: string; request: string; proposal: RebindProposal } | null>(null)
const rebindError = ref('')
async function prepareRebind() {
  const root = workspace.active?.path,
    record = entry.value,
    ticket = generation
  if (!root || !record || !available.value) return
  busy.value = true
  try {
    const res = await gateway.resourceRequest('queryContinuations', root, {
      operation: 'prepare_rebind',
      id: record.task.id,
    })
    if (ticket !== generation || entry.value?.task.id !== record.task.id) return
    if (!res.ok) {
      error.value = res.error
      return
    }
    const proposal = res.data as RebindProposal
    if (proposal.task !== record.task.id || !proposal.digest || !proposal.wait) {
      error.value = 'Invalid binding proposal; refresh the task.'
      return
    }
    rebindReview.value = { root, request: crypto.randomUUID(), proposal }
    rebindError.value = ''
  } finally {
    busy.value = false
  }
}
async function confirmRebind() {
  const review = rebindReview.value,
    ticket = generation
  if (!review || review.root !== workspace.active?.path || !available.value) return
  busy.value = true
  try {
    const res = await gateway.resourceRequest(
      'controlContinuation',
      review.root,
      {
        id: review.proposal.task,
        action: 'rebind',
        rebind_digest: review.proposal.digest,
      },
      review.proposal.revision,
      review.request,
    )
    if (ticket !== generation) return
    if (!res.ok) {
      rebindError.value = res.error
      return
    }
    rebindReview.value = null
    await load()
  } catch (cause) {
    rebindError.value = `Outcome unknown; retry this same confirmation to reconcile. ${String(cause)}`
  } finally {
    busy.value = false
  }
}
function requestStop() {
  const r = entry.value,
    root = workspace.active?.path
  if (r && root) {
    stopTarget.value = {
      root,
      id: r.task.id,
      revision: r.revision,
      goal: r.policy?.purpose || 'Task',
    }
    confirm.value = true
  }
}
interface LaunchReceipt {
  id: string
  state: string
  blueprint_name: string
  run_id: string | null
  attention: string | null
  resume_on_completion: boolean
  result: unknown
}
const launchReceipt = ref<LaunchReceipt | null>(null),
  launchError = ref(''),
  execution = useExecutionStore()
let launchQuery = 0
async function loadLaunch() {
  const id = entry.value?.launch_intent,
    root = workspace.active?.path,
    ticket = ++launchQuery
  if (!id || !root) {
    launchReceipt.value = null
    launchError.value = ''
    return
  }
  const response = await gateway.resourceRequest('queryBlueprintLaunch', root, { id })
  if (
    ticket !== launchQuery ||
    entry.value?.launch_intent !== id ||
    workspace.active?.path !== root
  )
    return
  if (response.ok) {
    launchReceipt.value = response.data as LaunchReceipt
    launchError.value = ''
  } else launchError.value = response.error
}
watch(
  () => entry.value?.launch_intent,
  () => {
    launchReceipt.value = null
    void loadLaunch()
  },
)
async function openRun() {
  if (!launchReceipt.value?.run_id) return
  const target = launchReceipt.value.run_id
  await execution.reconcile(target)
  if (execution.current?.runId === target) openSurface('execution')
  else launchError.value = execution.error || 'The requested run is unavailable.'
}
const starting = ref(false),
  launching = ref(false),
  config = useConfigStore()
const models = computed(() => Object.keys(config.effective.llm?.models ?? {}))
let preferredSession = ''
let generation = 0
const available = computed(() => gateway.connected.value && !busy.value)
function start() {
  if (available.value && !launching.value) starting.value = true
}
defineExpose({ start, refresh: load, available, launching })
function load(append = false) {
  return updates.run(() => loadSnapshot(append))
}
async function loadSnapshot(append = false) {
  const root = workspace.active?.path
  if (!root) return
  const ticket = generation
  if (!rows.value.length) loading.value = true
  try {
    const collected: TaskRecord[] = append ? [...rows.value] : []
    const target = append ? 50 : Math.max(rows.value.length, 50)
    const findSession = preferredSession
    let cursor: string | null = append ? next.value : null
    do {
      const res = await gateway.resourceRequest('queryContinuations', root, {
        operation: 'list',
        after: cursor,
      })
      if (ticket !== generation || workspace.active?.path !== root) return
      if (!res.ok) {
        error.value = res.error
        return
      }
      const page = res.data as TaskPage
      for (const item of page.items) {
        if (!collected.some((r) => r.task.id === item.task.id)) collected.push(item)
      }
      cursor = page.next
      if (
        append ||
        (collected.length >= target &&
          (!findSession || collected.some((r) => r.task.session === findSession)))
      )
        break
    } while (cursor)
    rows.value = collected
    if (preferredSession) {
      const created = collected.find((r) => r.task.session === preferredSession)
      if (created) {
        selected.value = created.task.id
        preferredSession = ''
      }
    }
    next.value = cursor
    error.value = ''
    void loadLaunch()
  } finally {
    if (ticket === generation) loading.value = false
  }
}
watch(
  () => workspace.active?.path,
  () => {
    generation++
    rows.value = []
    selected.value = ''
    preferredSession = ''
    confirm.value = false
    starting.value = false
    launching.value = false
    taskPrepared.value = null
    stopTarget.value = null
    rebindReview.value = null
    rebindError.value = ''
  },
  { immediate: true },
)
const updates = useWorkspaceRefresh(() => loadSnapshot())
onUnmounted(() => {
  generation++
  launchQuery++
})
async function control(action: string) {
  const r = entry.value,
    root = workspace.active?.path
  if (!r || !root || !available.value) return
  const target =
    action === 'stop' ? stopTarget.value : { root, id: r.task.id, revision: r.revision, goal: '' }
  if (!target || target.root !== root) return
  const ticket = generation
  busy.value = true
  try {
    const res = await gateway.resourceRequest(
      'controlContinuation',
      root,
      { id: target.id, action },
      target.revision,
    )
    if (ticket !== generation) return
    if (!res.ok) {
      error.value = res.error
      return
    }
    const updated = res.data as TaskRecord
    rows.value = rows.value.map((x) => (x.task.id === updated.task.id ? updated : x))
    confirm.value = false
    error.value = ''
  } finally {
    busy.value = false
  }
}
async function openSession() {
  if (!entry.value) return
  const target = entry.value.task.session,
    root = workspace.active?.path,
    ticket = generation
  if (chat.streaming) {
    error.value = 'A chat turn is active. Finish or stop it before switching sessions.'
    return
  }
  await chat.refreshThreads()
  if (root !== workspace.active?.path || ticket !== generation) return
  await chat.switchTo(target)
  if (root === workspace.active?.path && chat.sessionId === target) openSurface('chat')
}
function prepareTaskPlan() {
  const record = entry.value,
    root = workspace.active?.path
  if (!record || !root || !allowed('new_plan') || chat.sessionUnavailable) return
  const text = `Prepare a new plan for this recorded task. Recheck current permissions, versions, expiry and shared limits; do not resume an old execution.\n\nTask reference (source data): ${record.task.id} · revision ${record.revision}\nGoal: ${record.policy?.purpose ?? 'Recorded task'}\nRecorded reason: ${facts.value?.reason ?? 'Review required'}`
  chat.draft = [chat.draft, text].filter(Boolean).join('\n\n')
  openSurface('chat')
}
async function startTask(input: {
  goal: string
  model: string
  maxTurns: number
  tokenLimit: number
  minutes: number
  detached: boolean
}) {
  const root = workspace.active?.path,
    ticket = generation
  if (!root || launching.value) return
  if (chat.streaming || chat.busy) {
    error.value = 'Finish or stop the active chat before starting another task.'
    return
  }
  if (!chat.composerInput?.canSubmit) {
    error.value = 'Reconcile the pending input in Chat before starting a task.'
    return
  }
  launching.value = true
  error.value = ''
  try {
    if (!taskPrepared.value) {
      const previous = chat.draftEntity
      if (!(await useComposerInputsStore().backup(root, previous))) {
        error.value = 'The current chat draft could not be protected.'
        return
      }
      if (root !== workspace.active?.path || ticket !== generation || chat.draftEntity !== previous)
        return
      await chat.clear()
      if (root !== workspace.active?.path || ticket !== generation) return
      taskPrepared.value = { root, entity: chat.draftEntity }
    }
    if (taskPrepared.value.root !== root || taskPrepared.value.entity !== chat.draftEntity) {
      error.value = 'The destination conversation changed. Review it in Chat before starting.'
      return
    }
    chat.draft = input.goal
    const accepted = await chat.send(
      input.goal,
      {
        model: input.model,
        max_tokens: Math.max(
          1,
          Math.min(
            4096,
            config.effective.llm?.models?.[input.model]?.context_window_output ?? 4096,
          ),
        ),
        permission_mode: 'ask',
        continuation_policy: {
          purpose: input.goal,
          detached: input.detached,
          max_turns: input.maxTurns,
          token_limit: input.tokenLimit,
          expires_ms: Date.now() + input.minutes * 60000,
        },
      },
      'normal',
      () => {
        if (ticket === generation && workspace.active?.path === root) {
          preferredSession = taskPrepared.value?.entity ?? ''
          starting.value = false
          void load()
        }
      },
    )
    if (ticket !== generation || workspace.active?.path !== root) return
    if (!accepted)
      error.value =
        chat.sendError ||
        chat.composerInput?.error ||
        'The task was not confirmed. Review the retained input in Chat.'
    else if (chat.sessionId) preferredSession = chat.sessionId
    await load()
  } catch (cause) {
    if (ticket === generation) error.value = String(cause)
  } finally {
    if (ticket === generation) launching.value = false
  }
}

function reviewApproval(request: {
  request_id: string
  detail: string
  content_digest?: string
  target?: import('@/core/approval-decision').ApprovalTarget
}) {
  if (!chat.approval)
    chat.approval = {
      requestId: request.request_id,
      detail: request.detail,
      contentDigest: request.content_digest,
      target: request.target,
    }
}
</script>
<template>
  <section class="nx-live">
    <p v-if="updates.error" class="nx-live-alert" role="status">
      Live Updates Disconnected. {{ updates.error }}
    </p>
    <PageHeader v-if="!embedded" title="Tasks">
      <template #help>
        <InlineHelp
          text="Saved waits and bounded automatic turns. Current permissions are rechecked for every turn."
        />
      </template>
      <div class="nx-live-actions" style="margin: 0">
        <button class="btn btn-primary" :disabled="!available || launching" @click="start">
          <Plus :size="15" />New task</button
        ><button
          class="btn btn-outline"
          :disabled="!available"
          aria-label="Refresh tasks"
          @click="load()"
        >
          <RefreshCw :size="16" />
        </button>
      </div>
    </PageHeader>
    <p v-if="!gateway.connected.value" role="status" class="nx-live-alert">
      Disconnected. Showing last saved state; controls are unavailable.
    </p>
    <p v-if="error" role="alert" class="nx-live-alert">{{ error }}</p>
    <div v-if="loading" class="nx-live-empty" role="status">Loading tasks…</div>
    <div v-else-if="!entry" class="nx-live-empty">No tasks yet. Choose New task to begin.</div>
    <div v-else class="nx-live-layout">
      <aside>
        <button
          v-for="r in rows"
          :key="r.task.id"
          class="nx-live-row"
          :class="{ selected: r.task.id === entry.task.id }"
          @click="selected = r.task.id"
        >
          <strong>{{ r.policy?.purpose ?? 'Task' }}</strong
          ><span>{{ stateLabel(r.task.state) }}</span></button
        ><button v-if="next" class="btn btn-outline" @click="load(true)">Load more</button>
      </aside>
      <div class="nx-live-detail">
        <div class="nx-live-between">
          <h2>{{ entry.policy?.purpose ?? 'Task' }}</h2>
          <button class="btn btn-outline" @click="openSession">
            Open session <ArrowUpRight :size="14" />
          </button>
        </div>
        <span class="nx-live-badge">{{ stateLabel(entry.task.state) }}</span>
        <p v-if="launchError" class="nx-live-alert" role="alert">{{ launchError }}</p>
        <section v-if="launchReceipt" class="nx-live-source" style="display: block">
          <div class="nx-live-between">
            <strong>{{ launchReceipt.blueprint_name }}</strong
            ><span>{{ stateLabel(launchReceipt.state) }}</span>
          </div>
          <p v-if="launchReceipt.attention" class="nx-live-alert">{{ launchReceipt.attention }}</p>
          <p class="text-muted" style="margin: 12px 0">
            {{
              launchReceipt.resume_on_completion
                ? 'Completion returns to this conversation within its authorized limits.'
                : 'Result retained for review; no automatic conversation turn.'
            }}
          </p>
          <button
            v-if="launchReceipt.run_id"
            class="btn btn-outline"
            :disabled="!available"
            @click="openRun"
          >
            Open execution <ArrowUpRight :size="14" />
          </button>
          <details v-if="launchReceipt.result" class="nx-live-disclosure">
            <summary>Recorded run result</summary>
            <pre>{{ JSON.stringify(launchReceipt.result, null, 2) }}</pre>
          </details>
        </section>
        <section
          v-if="facts"
          class="nx-live-source"
          style="display: block"
          aria-label="Task next action"
        >
          <strong>{{ facts.reason }}</strong>
          <p>{{ facts.next_action }}</p>
          <button
            v-if="allowed('rebind')"
            class="btn"
            :disabled="!available"
            @click="prepareRebind"
          >
            Review new binding
          </button>
          <button
            v-if="
              allowed('new_plan') &&
              !Object.values(entry.task.sources).some((source) => !!source.terminal)
            "
            class="btn"
            :disabled="chat.sessionUnavailable"
            @click="prepareTaskPlan"
          >
            Prepare new plan in draft
          </button>
          <button v-if="allowed('open_settings')" class="btn" @click="openSurface('settings')">
            Review settings
          </button>
        </section>
        <p v-else-if="entry.task.attention" class="nx-live-alert">{{ entry.task.attention }}</p>
        <WorkflowResult
          v-for="source in Object.values(entry.task.sources).filter((source) => !!source.terminal)"
          :key="source.id"
          :reference="{ kind: 'task', taskId: entry.task.id, sourceId: source.id, digest: '' }"
          :label="source.kind"
          source-label="Result from the original task conversation"
          :allow-new-plan="allowed('new_plan')"
        />
        <div
          v-for="source in Object.values(entry.task.sources)"
          :key="source.id"
          class="nx-live-source"
        >
          <Terminal :size="18" /><strong
            >{{ source.kind }} ·
            {{
              source.kind === 'timer'
                ? source.id.slice(0, 12) + ' · ' + source.id.split(':').slice(-2).join('.')
                : source.id.slice(0, 12)
            }}</strong
          ><span>{{ source.terminal ? stateLabel(source.terminal) : 'Running / pending' }}</span>
        </div>
        <details class="nx-live-disclosure">
          <summary>Wait policy &amp; remaining budget</summary>
          <dl class="nx-live-facts">
            <dt>Waiting policy</dt>
            <dd>
              {{ entry.task.wait?.mode ?? 'No active wait'
              }}<span v-if="entry.task.wait">
                · until
                {{
                  new Date(entry.task.wait.deadline_ms).toLocaleString(undefined, {
                    timeZoneName: 'short',
                  })
                }}</span
              >
            </dd>
            <dt>Remaining budget</dt>
            <dd>
              {{ Math.max(0, entry.task.budget.max_turns - entry.task.budget.used_turns) }} turns ·
              {{
                Math.max(
                  0,
                  entry.task.budget.token_limit -
                    entry.task.budget.consumed_tokens -
                    entry.task.budget.reserved_tokens,
                ).toLocaleString()
              }}
              tokens<span v-if="entry.task.budget.unknown_usage">
                · usage unknown, continuation blocked</span
              >
            </dd>
          </dl>
        </details>
        <details class="nx-live-disclosure">
          <summary>Task details and source results</summary>
          <p>Session {{ entry.task.session }} · generation {{ entry.task.generation }}</p>
          <p v-if="entry.task.attention">Recorded reason: {{ entry.task.attention }}</p>
          <p v-if="facts">
            Reason code: {{ facts.reason_code }} · host revision {{ facts.target.revision }}
          </p>
          <pre>{{
            JSON.stringify(
              { params: entry.task.wait?.params, sources: entry.task.sources },
              null,
              2,
            )
          }}</pre>
        </details>
        <div
          v-for="request in entry.pending_approvals ?? []"
          :key="request.request_id"
          class="nx-live-alert"
        >
          This task needs your approval.
          <button
            class="btn btn-outline"
            :disabled="!!chat.approval"
            @click="reviewApproval(request)"
          >
            Review requested action
          </button>
        </div>
        <div class="nx-live-actions">
          <button
            class="btn btn-outline"
            :disabled="!available || !allowed(entry.task.paused ? 'resume' : 'pause')"
            @click="control(entry.task.paused ? 'resume' : 'pause')"
          >
            <Play v-if="entry.task.paused" :size="15" /><Pause v-else :size="15" />{{
              entry.task.paused ? 'Resume automatic continuation' : 'Pause automatic continuation'
            }}</button
          ><button
            class="btn nx-live-danger"
            :disabled="!available || ['stopped', 'completed'].includes(entry.task.state)"
            @click="requestStop"
          >
            <Square :size="14" />Stop task
          </button>
        </div>
      </div>
    </div>
    <div
      v-if="confirm"
      class="nx-live-overlay"
      @click.self="confirm = false"
      @keydown.esc="confirm = false"
    >
      <section
        v-dialog-focus
        role="dialog"
        aria-modal="true"
        aria-label="Stop task"
        class="nx-live-modal"
      >
        <h2>Stop This Task?</h2>
        <p>{{ stopTarget?.goal }}</p>
        <p>
          Invalidate future triggers and request cancellation of this task’s sources. Other tasks
          are unaffected. Unknown write effects remain blocked for review.
        </p>
        <div class="nx-live-actions">
          <button class="btn btn-outline" @click="confirm = false">Keep task</button
          ><button class="btn nx-live-danger" :disabled="busy" @click="control('stop')">
            Stop task
          </button>
        </div>
      </section>
    </div>
    <div v-if="rebindReview" class="nx-live-overlay" @keydown.esc="!busy && (rebindReview = null)">
      <section
        v-dialog-focus
        role="dialog"
        aria-modal="true"
        aria-label="Review new binding"
        class="nx-live-modal"
      >
        <h2>Continue with the updated conversation?</h2>
        <p>{{ rebindReview.proposal.goal }}</p>
        <p>
          The original task will use its conversation’s latest saved input and the same selected
          sources. Existing usage and consumed results are retained.
        </p>
        <blockquote>{{ rebindReview.proposal.latest_user_input }}</blockquote>
        <p>
          {{ rebindReview.proposal.budget.used_turns }} of
          {{ rebindReview.proposal.budget.max_turns }} turns used;
          {{ rebindReview.proposal.budget.consumed_tokens }} of
          {{ rebindReview.proposal.budget.token_limit }} tokens used.
        </p>
        <p>
          Authorization expires
          {{ new Date(rebindReview.proposal.policy.expires_ms).toLocaleString() }}.
          {{
            rebindReview.proposal.policy.detached
              ? 'Authorized background continuation remains enabled.'
              : 'The existing connection policy remains in effect.'
          }}
        </p>
        <details class="nx-live-disclosure">
          <summary>Conversation and sources</summary>
          <p>
            Session {{ rebindReview.proposal.session }} · revision
            {{ rebindReview.proposal.session_revision }}
          </p>
          <p v-for="source in rebindReview.proposal.wait.sources" :key="source">{{ source }}</p>
        </details>
        <p v-if="rebindError" class="nx-live-alert" role="alert">{{ rebindError }}</p>
        <div class="nx-live-actions">
          <button class="btn btn-outline" :disabled="busy" @click="rebindReview = null">
            Cancel
          </button>
          <button class="btn" :disabled="busy" @click="confirmRebind">
            Confirm binding and continue
          </button>
        </div>
      </section>
    </div>
    <TaskStartDialog
      v-if="starting"
      :models="models"
      :busy="launching"
      :error="models.length ? error : 'Configure a model in Settings before starting a task.'"
      @close="starting = false"
      @start="startTask"
    />
  </section>
</template>
