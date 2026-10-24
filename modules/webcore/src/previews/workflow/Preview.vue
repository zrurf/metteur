<script setup lang="ts">
import { ref, watch } from 'vue'
import WorkflowPlanner from '@/components/workflows/WorkflowPlanner.vue'
import WorkflowReview from '@/components/workflows/WorkflowReview.vue'
import { plannerIssues, type EvidenceStatus, type ReviewDecision } from '@/core/coding-workflows'
import { catalog, exampleInput, exampleReview } from './fixtures'
import { vDialogFocus } from '@/components/resources/dialog-focus'
import { useThemeStore } from '@/stores/theme'
import PageHeader from '@/components/PageHeader.vue'
const group = ref(
  new URLSearchParams(location.search).get('preview') === 'WD-PV-02' ? 'WD-PV-02' : 'WD-PV-01',
)
const screen = ref(group.value === 'WD-PV-02' ? 'review' : 'chat'),
  source = ref('chat'),
  fileReturn = ref('review')
const state = ref('Ready'),
  input = ref(exampleInput()),
  review = ref(exampleReview()),
  prepared = ref(false),
  error = ref(''),
  notice = ref(''),
  modal = ref(''),
  approved = ref(false),
  changed = ref(false),
  draft = ref('')
const filePath = ref('metrics/normalize.py'),
  fileText = ref(
    'def normalize(samples):\n    if not samples:\n        return []\n    mean = sum(samples) / len(samples)\n    return [value - mean for value in samples]\n',
  ),
  savedText = ref(fileText.value)
const saved = ref(false),
  ran = ref(group.value === 'WD-PV-02')
const theme = useThemeStore()
function toggleTheme() {
  theme.toggle()
}
watch(group, (value) => {
  screen.value = value === 'WD-PV-02' ? 'review' : 'chat'
  ran.value = value === 'WD-PV-02'
  error.value = ''
  notice.value = ''
})
watch(state, () => {
  error.value = ''
  notice.value = ''
  review.value = exampleReview()
  if (group.value === 'WD-PV-02') {
    const status = (
      { Failed: 'failed', Missing: 'missing', Skipped: 'skipped', Stale: 'stale' } as Record<
        string,
        EvidenceStatus
      >
    )[state.value]
    if (status) {
      review.value.criteria[1]!.status = status
      review.value.criteria[1]!.reason = (
        {
          failed: 'The check returned exit code 1. Inspect the failure before preparing a fix.',
          missing: 'No committed check result is available.',
          skipped: 'This optional branch did not execute. It does not count as passed.',
          stale: 'The source changed after the check. The earlier pass is retained as history.',
        } as Record<string, string>
      )[status]!
    }
    if (state.value === 'Unknown') {
      review.value.can_review = false
      review.value.status = 'Unconfirmed'
      notice.value = 'Reconnect and reconcile before recording a decision.'
    }
  }
})
function preview() {
  error.value = plannerIssues(input.value).join(' ')
  if (!error.value) prepared.value = true
}
function create() {
  if (state.value === 'Save failed') {
    error.value = 'The file could not be saved. Your plan remains here; no execution started.'
    return
  }
  saved.value = true
  screen.value = 'blueprint'
  notice.value = 'Fixture blueprint saved. It has not run.'
}
function launch() {
  modal.value = 'launch'
  approved.value = false
  changed.value = false
}
function start() {
  source.value = screen.value
  screen.value = 'planner'
}
function openFile(path: string) {
  filePath.value = path
  fileReturn.value = screen.value
  screen.value = 'file'
}
function leaveFile() {
  if (fileText.value !== savedText.value) {
    modal.value = 'unsaved'
    return
  }
  screen.value = fileReturn.value
}
function saveFile() {
  if (state.value === 'Save failed') {
    error.value = 'File save failed. Your edits remain in the editor.'
    return
  }
  savedText.value = fileText.value
  notice.value = 'Fixture file saved. The previous check is now stale.'
  review.value.criteria[1]!.status = 'stale'
  review.value.criteria[1]!.reason =
    'The file changed after the check. Review a new result before accepting.'
  for (const c of review.value.criteria) {
    if (c.decision) c.decision.current = false
  }
}
function decide(value: ReviewDecision) {
  if (state.value === 'Unknown') {
    error.value = 'Decision outcome unknown. Reconcile the same request before submitting another.'
    return
  }
  if (state.value === 'Conflict') {
    error.value = 'A newer review exists. Reload it before confirming; your note is preserved.'
    return
  }
  const c = review.value.criteria.find((criterion) => criterion.id === value.criterion_id)!
  c.decision = {
    decision: value.decision,
    note: value.note,
    current: true,
    subject: 'Fixture user',
  }
  review.value.revision++
  notice.value = 'Fixture decision recorded. No execution or permission changed.'
}
function prepare() {
  const unmet = review.value.criteria.filter(
    (c) => c.phase === 'acceptance' && (!c.decision?.current || c.decision.decision !== 'accepted'),
  )
  draft.value = [
    draft.value,
    'Review the saved result for “Handle empty metric samples”.',
    'Unmet requirements: ' + unmet.map((c) => c.description + ' (' + c.status + ')').join('; '),
    'Prepare a new proposal using the existing task history and limits.',
  ]
    .filter(Boolean)
    .join('\n\n')
  screen.value = 'chat'
  notice.value = 'Follow-up added to the original conversation draft. Nothing was sent.'
}
</script>
<template>
  <div class="delivery-preview">
    <div class="preview-controls">
      <strong>Isolated fixture · r2</strong
      ><label
        >Preview<select v-model="group" class="input" aria-label="Preview group">
          <option>WD-PV-01</option>
          <option>WD-PV-02</option>
        </select></label
      ><label
        >Scenario<select v-model="state" class="input" aria-label="Fixture scenario">
          <option
            v-for="s in [
              'Ready',
              'Save failed',
              'Failed',
              'Missing',
              'Skipped',
              'Stale',
              'Unknown',
              'Conflict',
            ]"
            :key="s"
          >
            {{ s }}
          </option>
        </select></label
      ><button class="btn" @click="toggleTheme">Toggle theme</button
      ><span>No real files, runs or model calls</span>
    </div>
    <div v-if="!['planner', 'file'].includes(screen)" class="preview-context">
      <span
        >Metrics workspace /
        {{
          screen === 'chat'
            ? 'Original conversation'
            : screen === 'task'
              ? 'Linked task'
              : screen === 'supervision'
                ? 'Blueprint supervision'
                : screen === 'blueprint'
                  ? 'Saved blueprint'
                  : 'Run result'
        }}</span
      ><button v-if="screen !== 'chat'" class="btn" @click="screen = 'chat'">
        Back to original Chat</button
      ><button v-if="screen !== 'task'" class="btn" @click="screen = 'task'">
        View linked task
      </button>
    </div>
    <WorkflowPlanner
      v-if="screen === 'planner'"
      v-model="input"
      :catalog="catalog"
      :models="['Fixture model · no calls']"
      :prepared="prepared"
      :error="error"
      @changed="prepared = false"
      @preview="preview"
      @create="create"
      @cancel="screen = source"
    />
    <WorkflowReview
      v-else-if="screen === 'review'"
      :review="review"
      :error="error"
      :notice="notice"
      @decide="decide"
      @prepare="prepare"
      @file="openFile"
      @refresh="notice = 'Fixture evidence refreshed. No checks rerun.'"
    />
    <section v-else class="preview-surface">
      <p v-if="notice" role="status" class="preview-notice">{{ notice }}</p>
      <p v-if="error" role="alert">{{ error }}</p>
      <template v-if="screen === 'chat'">
        <PageHeader
          title="Handle empty metric samples"
          description="Original conversation · existing history and draft retained"
        />
        <article class="preview-message">
          <strong>You</strong>
          <p>
            Fix the empty-input error. Keep the normalize signature and both existing callers
            unchanged.
          </p>
        </article>
        <article class="preview-message">
          <strong>Assistant · fixture</strong>
          <p>
            We can reproduce the failure, make a scoped fix, and run the regression checks. Review
            the requirements before running the plan.
          </p>
          <div class="preview-actions">
            <button class="btn" @click="start">Start from template</button
            ><button class="btn" @click="openFile('metrics/normalize.py')">Open source file</button
            ><button v-if="saved" class="btn" @click="screen = 'blueprint'">
              Open saved blueprint</button
            ><button v-if="ran" class="btn" @click="screen = 'review'">Review run result</button>
          </div>
        </article>
        <label class="preview-composer"
          >Message draft<textarea
            v-model="draft"
            class="input"
            rows="4"
            placeholder="Keep writing in the original conversation"
          />
        </label>
        <p class="preview-muted">
          Preview: sending is disabled. Adding a result preserves existing draft text.
        </p>
        <button class="btn" disabled>Send · unavailable in fixture</button>
      </template>
      <template v-else-if="screen === 'blueprint'"
        ><PageHeader :title="input.name" description="Saved blueprint · revision 1 · not started" />
        <p>{{ input.goal }}</p>
        <ol class="preview-stages">
          <li>Verify the baseline</li>
          <li>Implement the scoped change</li>
          <li>Check the requirements and review evidence</li>
        </ol>
        <div class="preview-actions">
          <button class="btn btn-primary" @click="launch">Review &amp; run</button
          ><button class="btn" @click="screen = 'planner'">Back to plan preview</button>
        </div>
        <p class="preview-muted">
          This fixture shows the handoff. The real saved blueprint uses the existing editor and
          version confirmation.
        </p></template
      >
      <template v-else-if="screen === 'task'"
        ><PageHeader
          title="Fix empty-input normalization"
          description="Linked task · same history and limits"
        />
        <p>
          Task:
          {{
            ran
              ? 'Awaiting delivery review'
              : saved
                ? 'Plan ready; awaiting explicit run'
                : 'Preparing a plan'
          }}
        </p>
        <p>Source: Handle empty metric samples</p>
        <p>Completion policy: retain the result; wait for the user.</p>
        <div class="preview-actions">
          <button v-if="ran" class="btn btn-primary" @click="screen = 'review'">
            Review run result</button
          ><button v-else-if="saved" class="btn" @click="screen = 'blueprint'">
            Open saved blueprint</button
          ><button v-else class="btn" @click="start">Start from template</button
          ><button v-if="ran" class="btn" @click="screen = 'supervision'">Open supervision</button>
        </div>
        <details>
          <summary>Task history &amp; limits</summary>
          <p>
            Fixture task: original task retained. No new budget, duplicate task or automatic retry
            is created by review.
          </p>
        </details></template
      >
      <template v-else-if="screen === 'supervision'"
        ><PageHeader
          title="Blueprint supervision"
          description="Separate entry · original Chat is unchanged"
        />
        <p>Fixture run ended. No active execution to control.</p>
        <button class="btn" @click="screen = 'review'">Review run result</button></template
      >
      <template v-else-if="screen === 'file'"
        ><PageHeader :title="filePath" description="Fixture editor · current working file"
          ><button class="btn" @click="leaveFile">Back to result or source</button></PageHeader
        >
        <p class="preview-muted">
          {{ fileText === savedText ? 'Saved fixture content' : 'Unsaved changes' }} · editing does
          not rerun checks.
        </p>
        <label
          >File content<textarea
            v-model="fileText"
            class="input preview-code"
            rows="14"
            spellcheck="false"
          />
        </label>
        <div class="preview-actions">
          <button class="btn btn-primary" :disabled="fileText === savedText" @click="saveFile">
            Save file · fixture</button
          ><button class="btn" @click="modal = 'file:' + filePath">View checked diff</button>
        </div></template
      >
    </section>
    <div v-if="modal" class="preview-overlay" @click.self="modal = ''">
      <section
        v-dialog-focus
        class="preview-dialog"
        role="dialog"
        aria-modal="true"
        :aria-label="
          modal === 'launch'
            ? 'Review saved blueprint'
            : modal === 'draft'
              ? 'Follow-up draft'
              : 'Evidence file'
        "
      >
        <header>
          <h2>
            {{
              modal === 'launch'
                ? 'Review saved blueprint'
                : modal === 'draft'
                  ? 'Follow-up draft'
                  : 'Evidence file'
            }}
          </h2>
          <button class="btn" @click="modal = ''">Close dialog</button>
        </header>
        <template v-if="modal === 'launch'"
          ><p>{{ input.name }}</p>
          <p>{{ input.goal }}</p>
          <p>Blueprint revision 1 · selected model and acceptance checks frozen.</p>
          <p>
            Original conversation is retained. Completion keeps the result; no automatic follow-up.
          </p>
          <details>
            <summary>Scope, checks &amp; permissions</summary>
            <p>{{ input.scope_paths.join(', ') }}</p>
            <ul>
              <li v-for="check in input.checks" :key="check.id">
                {{ check.description }} — {{ check.command }}
              </li>
            </ul>
            <p>
              Existing file and command permissions remain in force. This confirmation does not
              approve later privileged actions.
            </p>
          </details>
          <label
            ><input v-model="approved" type="checkbox" /> Confirm this version, handoff scope and
            completion policy</label
          ><label class="preview-muted"
            ><input v-model="changed" type="checkbox" /> Fixture: change a dependency after
            review</label
          >
          <p v-if="changed" role="alert">
            The selected version changed. Review a new proposal before running.
          </p>
          <button
            class="btn btn-primary"
            :disabled="!approved || changed"
            @click="
              modal = ''
              ran = true
              screen = 'chat'
              notice =
                'Fixture run completed. Original Chat retained; open the result or linked task when ready.'
            "
          >
            Confirm run · fixture
          </button></template
        >
        <template v-else-if="modal === 'unsaved'"
          ><p>Keep editing, or explicitly discard the unsaved fixture changes.</p>
          <button class="btn" @click="modal = ''">Keep editing</button
          ><button
            class="btn"
            @click="
              fileText = savedText
              modal = ''
              screen = fileReturn
            "
          >
            Discard changes
          </button></template
        >
        <template v-else-if="modal === 'draft'"
          ><p>Destination: original conversation. Sending is a separate action.</p>
          <textarea v-model="draft" class="input" rows="7" /><button
            class="btn"
            @click="
              modal = ''
              notice = 'Follow-up remains in the fixture draft. Nothing was sent.'
            "
          >
            Keep in draft
          </button></template
        >
        <template v-else
          ><p>{{ modal.slice(5) }}</p>
          <pre>
- mean = sum(samples) / len(samples)
+ if not samples:
+     return []
+ mean = sum(samples) / len(samples)</pre>
          <p class="preview-muted">
            Fixture diff · checked version is distinct from the current file.
          </p></template
        >
      </section>
    </div>
  </div>
</template>
<style scoped>
.delivery-preview {
  height: 100%;
  overflow: auto;
  background: var(--background);
  color: var(--foreground);
}
.preview-controls {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 14px;
  padding: 12px 20px;
  border-bottom: 1px solid var(--border);
  font-size: 13px;
}
.preview-controls label {
  display: flex;
  align-items: center;
  gap: 7px;
}
.preview-controls .input {
  font-size: 13px;
  min-height: 32px;
}
.preview-controls span,
.preview-muted {
  color: var(--muted-foreground);
}
.preview-empty {
  padding: 28px;
}
.preview-overlay {
  position: fixed;
  inset: 0;
  background: #0006;
  display: grid;
  place-items: center;
  padding: 20px;
  z-index: 100;
}
.preview-dialog {
  background: var(--background);
  border: 1px solid var(--border);
  border-radius: 12px;
  max-width: 680px;
  width: 100%;
  max-height: 88vh;
  overflow: auto;
  padding: 24px;
  display: grid;
  gap: 18px;
  font-size: 14px;
  line-height: 1.6;
}
.preview-dialog header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 16px;
}
.preview-dialog h2 {
  font-size: 18px;
  font-weight: 600;
}
.preview-dialog label {
  display: flex;
  align-items: center;
  gap: 10px;
}
.preview-dialog .input {
  width: 100%;
  font-size: 14px;
}
.preview-dialog pre {
  white-space: pre-wrap;
  padding: 15px;
  background: var(--muted);
}
summary {
  cursor: pointer;
}
button:focus-visible,
summary:focus-visible {
  outline: 2px solid var(--primary);
  outline-offset: 3px;
}
.preview-surface {
  max-width: 1040px;
  margin: auto;
  padding: 24px 28px;
  font-size: 14px;
  line-height: 1.6;
  display: grid;
  gap: 18px;
}
.preview-surface :deep(.surface-header) {
  padding: 0;
}
.preview-context,
.preview-actions {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 12px;
}
.preview-context {
  padding: 12px 28px;
  font-size: 13px;
  border-bottom: 1px solid var(--border);
  color: var(--muted-foreground);
}
.preview-message {
  padding: 16px 0;
  border-bottom: 1px solid var(--border);
}
.preview-message p {
  margin: 10px 0 16px;
}
.preview-surface label {
  display: grid;
  gap: 8px;
}
.preview-surface textarea.input {
  width: 100%;
  height: auto;
  font-size: 14px;
  line-height: 1.6;
  padding: 12px;
}
.preview-code {
  font-family: var(--font-mono);
  white-space: pre;
}
.preview-notice {
  padding: 12px 16px;
  background: var(--muted);
  border-left: 3px solid var(--primary);
}
.btn:not(.btn-primary) {
  color: var(--foreground);
}
.preview-stages {
  list-style: decimal;
  padding-left: 24px;
  display: grid;
  gap: 14px;
}
@media (max-width: 700px) {
  .preview-surface {
    padding: 18px 14px;
  }
}
</style>
