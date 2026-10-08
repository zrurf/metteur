<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import {
  Activity,
  BarChart3,
  Check,
  CircleDot,
  ListChecks,
  Loader,
  Network,
  Terminal,
  X,
} from '@lucide/vue'
import { useExecutionStore } from '@/stores/execution'
import { gateway } from '@/core'
import ExecutionBlackboard from './execution/ExecutionBlackboard.vue'
import ExecTreeRow from './ExecTreeRow.vue'
import type { EventLine } from '@/stores/execution'

/**
 * Execution audit docked to the blueprint canvas (Unreal-style debug trail).
 *
 * Three tabs live side by side with the running graph:
 * - Log: the raw event stream, tone-coded by action;
 * - Context: live LLM context composition from `context` events;
 * - Node: the audit facts of a selected node (outputs, tokens, message, times).
 * The parent keeps the drawer visibility and the node selection; everything
 * displayed here is read from the execution store.
 */

const props = defineProps<{
  /** Whether the drawer is visible (v-model). */
  open: boolean
  /** Node whose audit the Node tab shows; `null` falls back to the running node. */
  nodeId: string | null
}>()
const emit = defineEmits<{ (e: 'update:open', open: boolean): void }>()

const execution = useExecutionStore()
const tab = ref<'log' | 'context' | 'node' | 'tree' | 'todo' | 'blackboard'>('log')

async function openBlackboard() { await execution.reconcile(); tab.value = 'blackboard' }

const close = () => emit('update:open', false)

/** Progress line for the task list ("2/5 completed, 1 in progress"). */
const todoProgress = computed(() => {
  const list = execution.todos
  const completed = list.filter((t) => t.status === 'completed').length
  const active = list.filter((t) => t.status === 'in_progress').length
  return `${completed}/${list.length} completed · ${active} in progress`
})

/* Log tab                                                               */

const KIND_TONE: Record<EventLine['kind'], string> = {
  started: 'text-primary',
  finished: 'text-emerald-600 dark:text-emerald-400',
  message: 'text-muted-foreground',
  approval_request: 'text-amber-600 dark:text-amber-400',
  context: 'text-violet-500',
  todos: 'text-teal-600 dark:text-teal-400',
  node_data: 'text-sky-600 dark:text-sky-400',
  error: 'text-danger',
}

const logEl = ref<HTMLElement | null>(null)
watch(
  () => execution.events.length,
  () => {
    void nextTick(() => {
      logEl.value?.scrollTo({ top: logEl.value.scrollHeight })
    })
  },
)

/* Context tab                                                            */

const REGION_COLOR: Record<string, string> = {
  system: '#8b5cf6',
  user: '#3f8cff',
  assistant: '#2fbf8f',
  tool: '#f2994a',
}

const regionName = (r: string): string =>
  ({ system: 'System / Prompt', user: 'User prompt', assistant: 'Assistant', tool: 'Tool results' })[r] ?? r

/** The inspected node: selection wins, else the running node. */
const inspectedId = computed(() => props.nodeId ?? execution.runningNodeId)

/** Context regions for the inspected node, falling back to the latest. */
const usageList = computed(() => execution.contextOf(inspectedId.value) ?? [])

const barSegments = computed(() => {
  const total = Math.max(execution.contextTotal, 1)
  return usageList.value.map((r) => ({
    region: r.region,
    color: REGION_COLOR[r.region] ?? '#8f8fa8',
    pct: Math.round((r.tokens / total) * 1000) / 10,
    label: regionName(r.region),
  }))
})

/* Node tab                                                               */

/** Audit data of the node under inspection. */
const audit = computed(() => {
  const id = inspectedId.value
  return id ? (execution.nodeAudits.get(id) ?? null) : null
})

const auditLabel = computed(() => inspectedId.value ?? '—')
</script>

<template>
  <div
    class="glass absolute bottom-2 right-2 z-20 flex w-105 max-w-[92%] flex-col overflow-hidden rounded-xl border border-divider shadow-card"
    style="height: min(58%, 520px)"
  >
    <ExecutionBlackboard v-if="tab === 'blackboard'" class="order-last overflow-auto" :run-id="execution.runId" :connected="execution.connected && gateway.connected.value" :revision="execution.current?.updatedAt" />
    <!-- Header -->
    <div class="flex h-8 shrink-0 items-center gap-2 border-b border-divider px-2.5">
      <Activity class="h-3.5 w-3.5 text-muted-foreground" />
      <span class="text-[11px] font-semibold uppercase tracking-wider text-subtle">Audit</span>
      <button class="text-[11px]" :class="{ 'text-primary': tab === 'blackboard' }" @click="openBlackboard">Blackboard</button>
      <span
        class="chip ml-1"
        :class="{
          'bg-primary/15 text-primary': execution.status === 'running',
          'bg-amber-500/15 text-amber-600 dark:text-amber-400': execution.status === 'paused',
          'bg-danger-soft text-danger': ['cancelled', 'failed'].includes(execution.status),
          'text-muted-foreground': !['running', 'paused', 'cancelled', 'failed'].includes(execution.status),
        }"
      >
        <span
          v-if="execution.status === 'running'"
          class="h-1.5 w-1.5 animate-pulse rounded-full bg-primary"
        />
        {{ execution.status }}
      </span>

      <div class="ml-auto flex items-center gap-1">
        <button
          class="btn-icon h-6! w-6!"
          type="button"
          :class="tab === 'log' ? 'bg-hover text-foreground' : ''"
          title="Event log"
          @click="tab = 'log'"
        >
          <Terminal class="h-3.5 w-3.5" />
        </button>
        <button
          class="btn-icon h-6! w-6!"
          type="button"
          :class="tab === 'context' ? 'bg-hover text-foreground' : ''"
          title="Context usage"
          @click="tab = 'context'"
        >
          <BarChart3 class="h-3.5 w-3.5" />
        </button>
        <button
          class="btn-icon h-6! w-6!"
          type="button"
          :class="tab === 'node' ? 'bg-hover text-foreground' : ''"
          title="Node audit"
          @click="tab = 'node'"
        >
          <CircleDot class="h-3.5 w-3.5" />
        </button>
        <button
          class="btn-icon h-6! w-6!"
          type="button"
          :class="tab === 'tree' ? 'bg-hover text-foreground' : ''"
          title="Agent execution tree"
          @click="tab = 'tree'"
        >
          <Network class="h-3.5 w-3.5" />
        </button>
        <button
          class="btn-icon h-6! w-6!"
          type="button"
          :class="tab === 'todo' ? 'bg-hover text-foreground' : ''"
          title="Task list"
          @click="tab = 'todo'"
        >
          <ListChecks class="h-3.5 w-3.5" />
        </button>
        <button class="btn-icon h-6! w-6!" type="button" title="Close audit" @click="close">
          <X class="h-3.5 w-3.5" />
        </button>
      </div>
    </div>

    <!-- Log tab -->
    <ul
      v-if="tab === 'log'"
      ref="logEl"
      class="min-h-0 flex-1 overflow-y-auto px-1.5 py-1 font-mono text-[11.5px] leading-5"
    >
      <li v-for="(ev, i) in execution.events" :key="i" class="flex gap-2.5 px-1.5 py-0.5">
        <span class="w-16 shrink-0" :class="KIND_TONE[ev.kind]">{{ ev.kind }}</span>
        <span class="w-30 shrink-0 truncate text-muted-foreground" :title="ev.nodeId">{{
          ev.nodeId
        }}</span>
        <span class="min-w-0 flex-1 wrap-break-word">{{ ev.message }}</span>
      </li>
      <li v-if="execution.events.length === 0" class="px-1.5 py-1 text-muted-foreground">
        No events yet — press Run.
      </li>
    </ul>

    <!-- Context tab -->
    <div v-else-if="tab === 'context'" class="min-h-0 flex-1 overflow-y-auto p-3">
      <p v-if="!execution.contextUsage" class="text-[11.5px] text-muted-foreground">
        Waiting for a Call LLM node…
      </p>
      <template v-else>
        <p class="mb-1.5 flex items-baseline justify-between">
          <span class="text-[11px] text-muted-foreground">Estimated tokens</span>
          <span class="text-[12.5px] font-semibold">{{
            execution.contextTotal.toLocaleString()
          }}</span>
        </p>
        <div class="flex h-2.5 w-full overflow-hidden rounded-full" :aria-label="'Context regions'">
          <div
            v-for="s in barSegments"
            :key="s.region"
            class="h-full transition-all duration-300"
            :style="{ width: s.pct + '%', background: s.color }"
            :title="`${s.label}: ${s.pct}%`"
          />
        </div>
        <ul class="mt-3 space-y-1.5">
          <li v-for="r in usageList" :key="r.region" class="flex items-center gap-2">
            <span
              class="h-2.5 w-2.5 shrink-0 rounded-sm"
              :style="{ background: REGION_COLOR[r.region] ?? '#8f8fa8' }"
            />
            <span class="min-w-0 flex-1 truncate text-[11.5px] text-muted-foreground">{{
              regionName(r.region)
            }}</span>
            <span class="shrink-0 text-[11px] tabular-nums">{{ r.tokens.toLocaleString() }}</span>
          </li>
        </ul>
        <p v-if="execution.contextNode" class="mt-2 text-[10.5px] text-subtle">
          snapshot from {{ execution.contextNode }}
        </p>
      </template>
    </div>

    <!-- Node tab -->
    <div v-else-if="tab === 'node'" class="min-h-0 flex-1 overflow-y-auto p-3">
      <p class="panel-heading mb-1.5">{{ auditLabel }}</p>
      <div v-if="audit" class="space-y-2.5">
        <p v-if="audit.message" class="text-[11.5px] leading-relaxed text-muted-foreground">
          {{ audit.message }}
        </p>
        <p v-if="audit.tokens" class="text-[11px] text-muted-foreground">
          {{ audit.tokens.toLocaleString() }} tokens
        </p>
        <p
          v-if="audit.startedAt || audit.finishedAt"
          class="text-[10.5px] text-subtle"
        >
          started {{ audit.startedAt ? new Date(audit.startedAt).toLocaleTimeString() : '—' }}
          · finished
          {{ audit.finishedAt ? new Date(audit.finishedAt).toLocaleTimeString() : '—' }}
        </p>
        <pre
          v-if="Object.keys(audit.outputs).length"
          class="overflow-auto rounded-md bg-surface-muted p-2 text-[10.5px] leading-relaxed text-muted-foreground"
          >{{ JSON.stringify(audit.outputs, null, 2) }}</pre
        >
      </div>
      <p v-else class="text-[11.5px] text-muted-foreground">
        Select a node on the canvas to inspect its audit data.
      </p>
    </div>

    <!-- Task-list tab -->
    <div v-else-if="tab === 'todo'" class="min-h-0 flex-1 overflow-y-auto p-3">
      <p v-if="execution.todos.length === 0" class="text-[11.5px] text-muted-foreground">
        No task list — the agent records one when a task has multiple steps.
      </p>
      <template v-else>
        <p class="mb-2 text-[11px] text-muted-foreground">
          {{ todoProgress }}
        </p>
        <ul class="space-y-1">
          <li
            v-for="(todo, index) in execution.todos"
            :key="index"
            class="flex items-start gap-2 text-[12px] leading-5"
          >
            <Check
              v-if="todo.status === 'completed'"
              class="mt-0.5 h-3.5 w-3.5 shrink-0 text-emerald-600 dark:text-emerald-400"
            />
            <Loader
              v-else-if="todo.status === 'in_progress'"
              class="mt-0.5 h-3.5 w-3.5 shrink-0 animate-spin text-primary"
            />
            <span v-else class="mt-0.5 h-3.5 w-3.5 shrink-0 rounded-full border border-border" />
            <span
              :class="
                todo.status === 'completed'
                  ? 'text-muted-foreground line-through'
                  : todo.status === 'in_progress'
                    ? 'font-medium'
                    : ''
              "
            >
              {{ todo.status === 'in_progress' && todo.activeForm ? todo.activeForm : todo.content }}
            </span>
          </li>
        </ul>
      </template>
    </div>

    <!-- Execution-tree tab -->
    <div v-else-if="tab === 'tree'" class="min-h-0 flex-1 overflow-y-auto p-3">
      <p
        v-if="!execution.tree || execution.tree.nodes.length === 0"
        class="text-[11.5px] text-muted-foreground"
      >
        No execution tree yet — run a blueprint to build the task hierarchy.
      </p>
      <div v-else class="space-y-1">
        <ExecTreeRow
          v-for="root in execution.tree.roots"
          :key="root"
          :node-id="root"
          :tree="execution.tree"
          :depth="0"
        />
      </div>
    </div>
  </div>
</template>
