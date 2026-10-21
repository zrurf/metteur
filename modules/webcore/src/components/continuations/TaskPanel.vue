<script setup lang="ts">
import InlineHelp from '@/components/resources/InlineHelp.vue'
import { computed, onUnmounted, ref, watch } from 'vue'
import {useExecutionStore} from '@/stores/execution'
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
interface LaunchReceipt { id:string; state:string; blueprint_name:string; run_id:string|null; attention:string|null; resume_on_completion:boolean; result:unknown }
const launchReceipt=ref<LaunchReceipt|null>(null),launchError=ref(''),execution=useExecutionStore()
let launchQuery=0
async function loadLaunch(){
 const id=entry.value?.launch_intent,root=workspace.active?.path,ticket=++launchQuery
 if(!id||!root){launchReceipt.value=null;launchError.value='';return}
 const response=await gateway.resourceRequest('queryBlueprintLaunch',root,{id})
 if(ticket!==launchQuery||entry.value?.launch_intent!==id||workspace.active?.path!==root)return
 if(response.ok){launchReceipt.value=response.data as LaunchReceipt;launchError.value=''}else launchError.value=response.error
}
watch(()=>entry.value?.launch_intent,()=>{launchReceipt.value=null;void loadLaunch()})
async function openRun(){if(!launchReceipt.value?.run_id)return;const target=launchReceipt.value.run_id;await execution.reconcile(target);if(execution.current?.runId===target)openSurface('execution');else launchError.value=execution.error||'The requested run is unavailable.'}
const starting = ref(false),
  launching = ref(false),
  config = useConfigStore()
const models = computed(() => Object.keys(config.effective.llm?.models ?? {}))
let preferredSession = ''
let generation = 0,
  refreshing = false
const available = computed(() => gateway.connected.value && !busy.value)
async function load(append = false) {
  const root = workspace.active?.path
  if (!root || refreshing) return
  const ticket = generation
  refreshing = true
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
    refreshing = false
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
    void load()
  },
  { immediate: true },
)
const timer = setInterval(() => {
  if (!busy.value && !confirm.value) void load()
}, 2000)
onUnmounted(() => {
  generation++
  clearInterval(timer)
  launchQuery++
})
async function control(action: string) {
  const r = entry.value,
    root = workspace.active?.path
  if (!r || !root || !available.value) return
  const ticket = generation
  busy.value = true
  try {
    const res = await gateway.resourceRequest(
      'controlContinuation',
      root,
      { id: r.task.id, action },
      r.revision,
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
  if (chat.streaming) {
    error.value = 'A chat turn is active. Finish or stop it before switching sessions.'
    return
  }
  await chat.refreshThreads()
  await chat.switchTo(entry.value.task.session)
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
  const root = workspace.active?.path
  if (!root || launching.value) return
  const ticket = generation
  launching.value = true
  error.value = ''
  const result = await gateway.sendChat(
    root,
    input.goal,
    [],
    () => {},
    {
      model: input.model,
      max_tokens: 4096,
      permission_mode: 'ask',
      continuation_policy: {
        purpose: input.goal,
        detached: input.detached,
        max_turns: input.maxTurns,
        token_limit: input.tokenLimit,
        expires_ms: Date.now() + input.minutes * 60000,
      },
    },
    (sessionId) => {
      if (ticket === generation) {
        preferredSession = sessionId
        starting.value = false
        void load()
      }
    },
    '',
    undefined,
    undefined,
    undefined,
    undefined,
    undefined,
    (request) => {
      if (ticket === generation && workspace.active?.path === root) chat.approval = request
    },
  )
  if (ticket === generation) {
    launching.value = false
    if (!result.ok) error.value = result.error
    await load()
  }
}
function reviewApproval(request: { request_id: string; detail: string }) {
  if (!chat.approval) chat.approval = { requestId: request.request_id, detail: request.detail }
}
</script>
<template>
  <section class="nx-live">
    <header class="nx-live-heading">
      <div>
        <h1>Task continuation <InlineHelp text="Saved waits and bounded automatic turns. Current permissions are rechecked for every turn." /></h1>
      </div>
      <div class="nx-live-actions" style="margin: 0">
        <button
          class="btn btn-primary"
          :disabled="!available || launching"
          @click="starting = true"
        >
          <Plus :size="15" />New task</button
        ><button class="btn" :disabled="!available" aria-label="Refresh tasks" @click="load()">
          <RefreshCw :size="16" />
        </button>
      </div>
    </header>
    <p v-if="!gateway.connected.value" role="status" class="nx-live-alert">
      Disconnected. Showing last saved state; controls are unavailable.
    </p>
    <p v-if="error" role="alert" class="nx-live-alert">{{ error }}</p>
    <div v-if="loading" class="nx-live-empty" role="status">Loading tasks…</div>
    <div v-else-if="!entry" class="nx-live-empty">
      No tasks yet. Choose New task to begin.
    </div>
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
        ><button v-if="next" class="btn" @click="load(true)">Load more</button>
      </aside>
      <div class="nx-live-detail">
        <div class="nx-live-between">
          <h2>{{ entry.policy?.purpose ?? 'Task' }}</h2>
          <button class="btn" @click="openSession">Open session <ArrowUpRight :size="14" /></button>
        </div>
        <span class="nx-live-badge">{{ stateLabel(entry.task.state) }}</span>
        <p v-if="launchError" class="nx-live-alert" role="alert">{{launchError}}</p>
        <section v-if="launchReceipt" class="nx-live-source" style="display:block">
          <div class="nx-live-between"><strong>{{launchReceipt.blueprint_name}}</strong><span>{{stateLabel(launchReceipt.state)}}</span></div>
          <p v-if="launchReceipt.attention" class="nx-live-alert">{{launchReceipt.attention}}</p>
          <p class="text-muted" style="margin:12px 0">{{launchReceipt.resume_on_completion ? 'Completion returns to this conversation within its authorized limits.' : 'Result retained for review; no automatic conversation turn.'}}</p>
          <button v-if="launchReceipt.run_id" class="btn" :disabled="!available" @click="openRun">Open execution <ArrowUpRight :size="14"/></button>
          <details v-if="launchReceipt.result" class="nx-live-disclosure"><summary>Recorded run result</summary><pre>{{JSON.stringify(launchReceipt.result,null,2)}}</pre></details>
        </section>
        <p v-if="entry.task.attention" class="nx-live-alert">{{ entry.task.attention }}</p>
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
        <details class="nx-live-disclosure">
          <summary>Task details and source results</summary><p>Session {{ entry.task.session }} · generation {{ entry.task.generation }}</p>
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
          <button class="btn" :disabled="!!chat.approval" @click="reviewApproval(request)">
            Review requested action
          </button>
        </div>
        <div class="nx-live-actions">
          <button
            class="btn"
            :disabled="
              !available || ['stopped', 'completed', 'needs_attention'].includes(entry.task.state)
            "
            @click="control(entry.task.paused ? 'resume' : 'pause')"
          >
            <Play v-if="entry.task.paused" :size="15" /><Pause v-else :size="15" />{{
              entry.task.paused ? 'Resume automatic continuation' : 'Pause automatic continuation'
            }}</button
          ><button
            class="btn nx-live-danger"
            :disabled="!available || ['stopped', 'completed'].includes(entry.task.state)"
            @click="confirm = true"
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
        <h2>Stop this task?</h2>
        <p>
          Invalidate future triggers and request cancellation of this task’s sources. Other tasks
          are unaffected. Unknown write effects remain blocked for review.
        </p>
        <div class="nx-live-actions">
          <button class="btn" @click="confirm = false">Keep task</button
          ><button class="btn nx-live-danger" :disabled="busy" @click="control('stop')">
            Stop task
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
