<script setup lang="ts">
import InlineHelp from '@/components/resources/InlineHelp.vue'
import { computed, onUnmounted, ref, watch } from 'vue'
import { CalendarClock, Plus, Pause, Play, Square, Pencil } from '@lucide/vue'
import { useSurfaceNavigation } from '@/lib/surface'
import { gateway } from '@/core'
import { useWorkspaceStore } from '@/stores/workspace'
import { vDialogFocus } from '@/components/resources/dialog-focus'
import type { TaskRecord, TaskPage, Schedule } from '@/core/continuations'
import './continuations.css'
const {openSurface}=useSurfaceNavigation()
const workspace = useWorkspaceStore(),
  tasks = ref<TaskRecord[]>([]),
  error = ref(''),
  busy = ref(false),
  editing = ref(false),
  taskId = ref(''),
  kind = ref('once'),
  zone = ref('Asia/Shanghai'),
  date = ref(new Date().toISOString().slice(0, 10)),
  time = ref('09:00'),
  minutes = ref(30),
  weekday = ref(0),
  maximum = ref(3),
  misfire = ref('coalesce_once'),
  consent = ref(false)
const editId = ref<string | null>(null)
const review = ref<{ digest: string; plan: Schedule; replaces_wait: boolean } | null>(null)
let generation = 0,
  pending: Record<string, unknown> | null = null,
  pendingRevision = 0
const selected = computed(() => tasks.value.find((t) => t.task.id === taskId.value))
const rows = computed(() =>
  tasks.value.flatMap((t) => t.schedules.map((s) => ({ task: t, schedule: s }))),
)
const eligible = computed(() => tasks.value.some(t => ['waiting','paused','ready'].includes(t.task.state) && t.has_saved_context))
let refreshing = false
const available = computed(() => gateway.connected.value && !busy.value)
async function load() {
  const root = workspace.active?.path
  if (!root || busy.value || refreshing) return
  refreshing = true
  try {
  const ticket = generation
  const all: TaskRecord[] = []
  let after: string | null = null
  do {
    const res = await gateway.resourceRequest('queryContinuations', root, {
      operation: 'list',
      after,
    })
    if (ticket !== generation) return
    if (!res.ok) {
      error.value = res.error
      return
    }
    const p = res.data as TaskPage
    all.push(...p.items)
    after = p.next
  } while (after)
  tasks.value = all
  error.value = ''
  } finally { refreshing = false }
}
watch(
  () => workspace.active?.path,
  () => {
    generation++
    tasks.value = []
    editing.value = false
    review.value = null
    void load()
  },
  { immediate: true },
)
const timer = setInterval(() => {
  if (!editing.value) void load()
}, 3000)
onUnmounted(() => {
  generation++
  clearInterval(timer)
})
watch([taskId, kind, zone, date, time, minutes, weekday, maximum, misfire], () => {
  review.value = null
  consent.value = false
  pending = null
})
function open() {
  editId.value = null
  taskId.value =
    tasks.value.find(
      (t) => ['waiting', 'paused', 'ready'].includes(t.task.state) && t.has_saved_context,
    )?.task.id ?? ''
  editing.value = true
  review.value = null
  consent.value = false
  error.value = ''
}

function displayTime(value: string, schedule?: Schedule) {
  const rule = schedule?.rule as Record<string, unknown> | undefined
  return new Date(value).toLocaleString(undefined, {
    timeZone: String(rule?.zone ?? 'UTC'),
    timeZoneName: 'short',
  })
}
function formatRule(schedule: Schedule) {
  const r = schedule.rule as Record<string, unknown>
  const clock = `${String(r.hour ?? 0).padStart(2, '0')}:${String(r.minute ?? 0).padStart(2, '0')}`
  switch (r.kind) {
    case 'once':
      return `Once · ${displayTime(String(r.at), schedule)}`
    case 'after':
      return `After duration · ${displayTime(String(r.deadline), schedule)}`
    case 'interval':
      return `Every ${Number(r.seconds) / 60} minutes`
    case 'daily':
      return `Daily at ${clock} · ${r.zone}`
    case 'weekly':
      return `${['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'][Number(r.weekday)]} at ${clock} · ${r.zone}`
    default:
      return 'Unknown rule'
  }
}
function edit(task: TaskRecord, schedule: Schedule) {
  open()
  editId.value = schedule.id
  taskId.value = task.task.id
  const r = schedule.rule as Record<string, unknown>
  kind.value = String(r.kind)
  zone.value = String(r.zone ?? 'UTC')
  maximum.value = schedule.max_occurrences
  misfire.value = schedule.misfire
  if (r.hour !== undefined)
    time.value = `${String(r.hour).padStart(2, '0')}:${String(r.minute).padStart(2, '0')}`
  if (r.weekday !== undefined) weekday.value = Number(r.weekday)
  if (r.seconds !== undefined) minutes.value = Number(r.seconds) / 60
  if (r.at) {
    const d = new Date(String(r.at))
    date.value = d.toISOString().slice(0, 10)
    time.value = d.toISOString().slice(11, 16)
  }
  if (r.deadline)
    minutes.value = Math.max(
      1,
      Math.ceil((new Date(String(r.deadline)).getTime() - Date.now()) / 60000),
    )
}

function utcFromLocal() {
  const [year, month, day] = date.value.split('-').map(Number),
    [hour, minute] = time.value.split(':').map(Number)
  const nominal = Date.UTC(year!, month! - 1, day!, hour!, minute!)
  const fmt = new Intl.DateTimeFormat('en-CA', {
    timeZone: zone.value,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    hourCycle: 'h23',
  })
  const matches: number[] = []
  for (let offset = -14 * 60; offset <= 14 * 60; offset += 15) {
    const instant = nominal - offset * 60000
    const p = Object.fromEntries(fmt.formatToParts(new Date(instant)).map((x) => [x.type, x.value]))
    if (
      Number(p.year) === year &&
      Number(p.month) === month &&
      Number(p.day) === day &&
      Number(p.hour) === hour &&
      Number(p.minute) === minute
    )
      matches.push(instant)
  }
  if (!matches.length)
    throw new Error('This local time does not exist in the selected zone. Choose another time.')
  return new Date(Math.min(...matches)).toISOString()
}
async function prepare() {
  const t = selected.value,
    root = workspace.active?.path
  if (!t || !root || !available.value) return
  busy.value = true
  const ticket = generation
  try {
    const [hour, minute] = time.value.split(':').map(Number)
    let rule: unknown
    if (kind.value === 'once') rule = { kind: 'once', at: utcFromLocal() }
    else if (kind.value === 'after')
      rule = { kind: 'after', deadline: new Date(Date.now() + minutes.value * 60000).toISOString() }
    else if (kind.value === 'interval')
      rule = {
        kind: 'interval',
        anchor: new Date(Date.now() + minutes.value * 60000).toISOString(),
        seconds: minutes.value * 60,
      }
    else
      rule = {
        kind: kind.value,
        zone: zone.value,
        hour,
        minute,
        ...(kind.value === 'weekly' ? { weekday: weekday.value } : {}),
      }
    pending = {
      task_id: t.task.id,
      id: editId.value ?? crypto.randomUUID(),
      rule,
      max_occurrences: ['once', 'after'].includes(kind.value) ? 1 : maximum.value,
      expires_at: new Date(t.policy?.expires_ms ?? Date.now()).toISOString(),
      misfire: misfire.value,
      params: editId.value
        ? (t.schedules.find((s) => s.id === editId.value)?.params ?? {})
        : { purpose: t.policy?.purpose ?? '' },
      confirmed_digest: null,
    }
    pendingRevision = t.revision
    const res = await gateway.resourceRequest('writeSchedule', root, pending, pendingRevision)
    if (ticket !== generation) return
    if (!res.ok) {
      error.value = res.error
      return
    }
    review.value = res.data as typeof review.value
    error.value = ''
  } catch (e) {
    error.value = String(e)
  } finally {
    busy.value = false
  }
}
async function save() {
  const root = workspace.active?.path
  if (!root || !pending || !review.value || !consent.value || !available.value) return
  const ticket = generation
  busy.value = true
  try {
    const res = await gateway.resourceRequest(
      'writeSchedule',
      root,
      { ...pending, confirmed_digest: review.value.digest },
      pendingRevision,
    )
    if (ticket !== generation) return
    if (!res.ok) {
      error.value = res.error
      return
    }
    editing.value = false
    review.value = null
    error.value = ''
  } finally {
    busy.value = false
  }
  await load()
}
async function control(t: TaskRecord, action: string) {
  const root = workspace.active?.path
  if (!root || !available.value) return
  busy.value = true
  try {
    const res = await gateway.resourceRequest(
      'controlContinuation',
      root,
      { id: t.task.id, action },
      t.revision,
    )
    if (!res.ok) error.value = res.error
  } finally {
    busy.value = false
  }
  await load()
}
</script>
<template>
  <section class="nx-live">
    <header class="nx-live-heading">
      <div>
        <h1>Schedules <InlineHelp text="Schedule a bounded continuation of an existing saved task and session." /></h1>
      </div>
      <button v-if="eligible" class="btn btn-primary" :disabled="!available" @click="open">
        <Plus :size="16" />New schedule
      </button>
    </header>
    <p v-if="!gateway.connected.value" class="nx-live-alert">
      Disconnected. Changes are unavailable.
    </p>
    <p v-if="error" role="alert" class="nx-live-alert">{{ error }}</p>
    <div v-if="!rows.length" class="nx-live-empty">
      <CalendarClock :size="30" />
      <h2>No schedules</h2>
      <p>Save a waiting task to schedule its next step.</p><button v-if="!eligible" class="btn" @click="openSurface('tasks')">Open tasks</button>
    </div>
    <article
      v-for="row in rows"
      :key="row.schedule.id"
      class="nx-live-detail"
      style="border: 1px solid var(--border); border-radius: 10px; margin-bottom: 16px"
    >
      <div class="nx-live-between">
        <h2>{{ row.task.policy?.purpose ?? 'Scheduled task' }}</h2>
        <span class="nx-live-badge">{{
          row.schedule.cancelled
            ? 'Stopped'
            : row.task.task.paused
              ? 'Paused'
              : row.schedule.next_due
                ? 'Scheduled'
                : 'Finished'
        }}</span>
      </div>
      <dl class="nx-live-facts">
        <dt>Rule</dt>
        <dd>{{ formatRule(row.schedule) }}</dd>
        <dt>Next occurrence</dt>
        <dd>
          {{
            row.schedule.next_due
              ? (row.task.task.paused ? 'Paused · ' : '') +
                displayTime(row.schedule.next_due, row.schedule)
              : 'Not scheduled'
          }}
        </dd>
        <dt>Missed triggers</dt>
        <dd>{{ row.schedule.misfire === 'coalesce_once' ? 'Coalesce once' : 'Skip' }}</dd>
        <dt>Bounded use</dt>
        <dd>
          {{ row.schedule.occurrences }} / {{ row.schedule.max_occurrences }} occurrences · expires
          {{ displayTime(row.schedule.expires_at, row.schedule) }}
        </dd>
        <dt>Session</dt>
        <dd>{{ row.task.task.session }}</dd>
      </dl>
      <div class="nx-live-actions">
        <button
          class="btn"
          :disabled="!available || !['waiting', 'paused', 'ready'].includes(row.task.task.state)"
          @click="edit(row.task, row.schedule)"
        >
          <Pencil :size="15" />Edit plan</button
        ><button
          class="btn"
          :disabled="!available || row.schedule.cancelled"
          @click="control(row.task, row.task.task.paused ? 'resume' : 'pause')"
        >
          <Play v-if="row.task.task.paused" :size="15" /><Pause v-else :size="15" />{{
            row.task.task.paused ? 'Resume' : 'Pause'
          }}</button
        ><button
          class="btn nx-live-danger"
          :disabled="!available || row.schedule.cancelled"
          @click="control(row.task, 'cancel_schedule')"
        >
          <Square :size="14" />Cancel future occurrences
        </button>
      </div>
    </article>
    <div
      v-if="editing"
      class="nx-live-overlay"
      @click.self="editing = false"
      @keydown.esc="editing = false"
    >
      <section
        v-dialog-focus
        class="nx-live-modal"
        role="dialog"
        aria-modal="true"
        aria-label="Schedule plan"
      >
        <div class="nx-live-between">
          <h2>Schedule a continuation</h2>
          <button class="btn" @click="editing = false">Close</button>
        </div>
        <p
          v-if="
            !tasks.some(
              (t) => ['waiting', 'paused', 'ready'].includes(t.task.state) && t.has_saved_context,
            )
          "
          class="nx-live-alert"
        >
          There is no saved waiting task. A schedule cannot silently create a different session or
          expand an existing task’s policy.
        </p>
        <div class="nx-live-form">
          <label
            >Task<select v-model="taskId" class="input" :disabled="!!editId">
              <option value="" disabled>Select a saved waiting task</option>
              <option
                v-for="t in tasks.filter(
                  (t) =>
                    ['waiting', 'paused', 'ready'].includes(t.task.state) && t.has_saved_context,
                )"
                :key="t.task.id"
                :value="t.task.id"
              >
                {{ t.policy?.purpose ?? t.task.id }}
              </option>
            </select></label
          ><label
            >Rule<select v-model="kind" class="input">
              <option value="once">Once</option>
              <option value="after">After duration</option>
              <option value="interval">Fixed interval</option>
              <option value="daily">Daily</option>
              <option value="weekly">Weekly</option>
            </select></label
          ><label v-if="!['after', 'interval'].includes(kind)"
            >Time zone<select v-model="zone" class="input">
              <option>Asia/Shanghai</option>
              <option>UTC</option>
              <option>America/New_York</option>
            </select></label
          ><label v-if="kind === 'once'"
            >Date<input v-model="date" type="date" class="input" /></label
          ><label v-if="['interval', 'after'].includes(kind)"
            >Minutes<input
              v-model.number="minutes"
              type="number"
              min="1"
              max="10080"
              class="input" /></label
          ><label v-else>Local time<input v-model="time" type="time" class="input" /></label
          ><label v-if="kind === 'weekly'"
            >Weekday<select v-model.number="weekday" class="input">
              <option
                v-for="(day, i) in [
                  'Monday',
                  'Tuesday',
                  'Wednesday',
                  'Thursday',
                  'Friday',
                  'Saturday',
                  'Sunday',
                ]"
                :key="day"
                :value="i"
              >
                {{ day }}
              </option>
            </select></label
          ><label v-if="!['once', 'after'].includes(kind)"
            >Maximum occurrences<input
              v-model.number="maximum"
              type="number"
              min="1"
              max="20"
              class="input" /></label
          ><label
            >Missed triggers<select v-model="misfire" class="input">
              <option value="coalesce_once">Coalesce once</option>
              <option value="skip">Skip</option>
            </select></label
          >
        </div>
        <p v-if="error" role="alert" class="nx-live-alert">{{ error }}</p>
        <div v-if="review" class="nx-live-alert">
          <div>
            <strong>Review the exact plan</strong>
            <p>
              Next: {{ review.plan.next_due }} (UTC)<br />Up to
              {{ review.plan.max_occurrences }} occurrences; task token budget remains shared.<br />Expires:
              {{ review.plan.expires_at }}<br />This replaces the task’s current wait. Requires a
              running daemon and open workspace.
            </p>
          </div>
        </div>
        <label v-if="review" class="nx-live-consent"
          ><input v-model="consent" type="checkbox" />Allow this exact plan within the existing task
          permissions and budget</label
        >
        <div class="nx-live-actions">
          <button class="btn" @click="editing = false">Cancel</button
          ><button
            v-if="!review"
            class="btn btn-primary"
            :disabled="!available || !selected"
            @click="prepare"
          >
            Review plan</button
          ><button v-else class="btn btn-primary" :disabled="!available || !consent" @click="save">
            Confirm schedule
          </button>
        </div>
      </section>
    </div>
  </section>
</template>
