<script setup lang="ts">
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useWorkspaceStore } from '@/stores/workspace'
import { useDocumentsStore } from '@/stores/documents'
import { fixture, fixtureControl, draftKey } from './ide-state'
import { wurl } from '@/lib/workspace-url'
import { pathByFileParam } from '@/lib/file-token'
const props = defineProps<{ mode: 'chat' | 'editor' | 'tasks' | 'dialog' | 'supervision' }>()
const router = useRouter(),
  route = useRoute(),
  workspace = useWorkspaceStore(),
  documents = useDocumentsStore()
const doc = computed(() =>
  workspace.active
    ? documents.get(
        workspace.active.path,
        pathByFileParam(String(route.query.f ?? '')) || 'metrics/README.md',
      )
    : null,
)
const blocked = computed(() =>
  [
    'Unknown Effect',
    'Budget Exhausted',
    'Policy Expired',
    'Permission Revoked',
    'Missing Source',
    'Stale Version',
  ].includes(fixture.state),
)
const reason = computed(
  () =>
    (
      ({
        'Unknown Effect':
          'A command may have changed files. Inspect journal and receipts before recovery.',
        'Budget Exhausted': 'Root budget exhausted. A new task cannot reset the ledger.',
        'Policy Expired': 'Policy expired. Prepare a new version-bound proposal.',
        'Permission Revoked': 'Permission revoked. Old authorization is invalid.',
        'Missing Source': 'Source conversation unavailable. Do not substitute another session.',
        'Stale Version': 'Blueprint changed after confirmation. Compare and reconfirm.',
        Disconnected: 'Ownership and control receipts are unknown. Reconnect and reconcile.',
        'Waiting Approval': 'Review the exact request and version in the approval dialog.',
      }) as Record<string, string>
    )[fixture.state] ??
    `Blueprint ${fixture.state.toLowerCase()}. Main-chat drafts remain editable.`,
)
function backup() {
  if (fixture.state === 'Disconnected') {
    fixture.notice = 'Offline: current changes are not backed up.'
    return
  }
  fixture.backup = doc.value?.content ?? ''
  fixture.notice = 'Host-confirmed snapshot · Fixture'
}
function recover() {
  if (!fixture.backup) {
    fixture.notice = 'No confirmed copy'
    return
  }
  if (doc.value) documents.edit(doc.value.workspace, doc.value.path, fixture.backup)
  fixture.reviewed = fixture.diskRevision
  fixture.dialog = 'Compare Changes'
}
function keep(mode: string) {
  if (mode === 'save' && fixture.reviewed !== fixture.diskRevision) {
    fixture.notice = 'Disk changed again. Review the latest version.'
    return
  }
  if (mode === 'disk' && doc.value)
    documents.edit(doc.value.workspace, doc.value.path, fixture.disk)
  fixture.notice =
    mode === 'both'
      ? 'Created README.recovered.md using create-only · Fixture'
      : mode === 'save'
        ? 'Conditional save confirmed · Fixture'
        : 'Disk version adopted into draft'
  fixture.dialog = ''
}
function addResult() {
  const key = draftKey(false)
  fixture.drafts[key] =
    (fixture.drafts[key] ?? '') +
    '\n[Result source: fixture-run / check-7 / revision 3]\n12 tests passed. Review parser.ts and api.ts.'
  fixture.notice = 'Result added to current draft. No model call. Send remains explicit.'
}
function compare() {
  fixture.reviewed = fixture.diskRevision
  fixture.dialog = 'Compare Changes'
}
function externalChange() {
  fixture.diskRevision++
  fixture.disk += '\nExternal change'
}
function reviewLatest() {
  fixture.reviewed = fixture.diskRevision
  fixture.notice = 'Latest disk version reviewed'
}
function copyOrphan() {
  fixture.drafts[draftKey(false)] = 'Recovered orphan text'
  fixture.notice = 'Text copied to draft. No session created or model called.'
}
function useSaved() {
  fixture.notice = 'Attached saved revision ' + fixture.diskRevision + '. Editor draft unchanged.'
  fixture.dialog = ''
}
function discardFixture() {
  fixture.backup = ''
  fixture.dialog = ''
  fixture.notice = 'Discarded exact copy; late fixture writes rejected'
}
function confirmLaunch() {
  fixture.notice = 'One fixture launch intent accepted; no real execution'
  fixture.dialog = ''
}
function stopFixture() {
  fixtureControl('Stop')
  fixture.dialog = ''
}
</script>
<template>
  <div class="wf-addon" :class="{ 'chat-column': mode === 'chat' }">
    <section v-if="mode === 'chat'" class="wf-run-card">
      <header>
        <strong>Validation Repair</strong><span class="wf-tag">{{ fixture.state }}</span>
      </header>
      <p>{{ reason }}</p>
      <div class="wf-actions">
        <button class="btn" @click="router.push(wurl('/execution'))">Open Supervision</button
        ><button class="btn" @click="router.push(wurl('/tasks'))">View Result</button
        ><button
          class="btn"
          @click="fixtureControl(fixture.state === 'Paused' ? 'Resume' : 'Pause')"
        >
          {{ fixture.state === 'Paused' ? 'Resume' : 'Pause' }}</button
        ><button class="btn" @click="fixture.dialog = 'Stop Run'">Stop</button>
      </div>
    </section>
    <div
      v-else-if="mode === 'editor'"
      class="flex flex-wrap items-center gap-2 border-b border-border px-3 py-1.5"
    >
      <span class="wf-tag">{{
        fixture.state === 'Disconnected'
          ? 'Not Backed Up · Offline'
          : doc?.content === fixture.backup && fixture.backup
            ? 'Backed Up on Host · Fixture'
            : 'Not Backed Up'
      }}</span>
      <button class="btn" @click="fixture.dialog = 'Recovery Copies'">Recovery Copies</button>
      <button class="btn" @click="fixture.dialog = 'Reference File'">Reference in Chat</button>
      <details class="wf-options">
        <summary>Draft Options</summary>
        <div class="wf-options-menu">
          <button class="btn" @click="backup">Back Up</button>
          <button class="btn" @click="compare">Compare</button>
          <button class="btn" @click="fixture.dialog = 'Discard Draft'">Discard Draft</button>
        </div>
      </details>
    </div>
    <section v-else-if="mode === 'tasks'">
      <div class="wf-run-card">
        <header>
          <strong>Repair Validation Across Files</strong
          ><span class="wf-tag">{{ fixture.state }}</span>
        </header>
        <p>{{ reason }}</p>
        <details>
          <summary>Source</summary>
          <p>
            {{
              fixture.source === 'Editor Direct Run'
                ? 'Independent run · No source session or automatic continuation'
                : 'Source Session A · Intent launch-17 · Endpoint turn-6'
            }}
          </p>
        </details>
        <div class="wf-actions">
          <button class="btn" @click="router.push(wurl('/execution'))">Open Execution</button
          ><button v-if="blocked" class="btn" @click="fixture.dialog = 'Recovery Diagnostics'">
            Inspect Reason & Evidence</button
          ><button class="btn" @click="fixture.dialog = 'Stop Run'">Stop</button>
          <details class="wf-options">
            <summary>Run Options</summary>
            <div class="wf-options-menu">
              <button class="btn" @click="fixture.dialog = 'Review Launch'">Review Launch</button>
              <button class="btn" :disabled="blocked" @click="fixtureControl('Resume')">
                Resume Execution
              </button>
            </div>
          </details>
        </div>
      </div>
      <div class="wf-results">
        <article>
          <h3>Execution</h3>
          <p>{{ fixture.state }}</p>
        </article>
        <article>
          <h3>Deterministic Checks</h3>
          <p>{{ fixture.state === 'Completed' ? '12 Passed · Exit 0' : 'Not Run / Pending' }}</p>
          <button
            class="wf-source-link"
            @click="fixture.notice = 'Fixture check-7: node --test · 12 passed · revision 3'"
          >
            Open Test Report
          </button>
        </article>
      </div>
      <details>
        <summary>Files & Artifacts</summary>
        <button
          v-for="file in ['src/parser.ts', 'src/api.ts', 'test-report.txt']"
          :key="file"
          class="wf-source-link"
          @click="fixture.notice = 'Immutable fixture evidence: ' + file + ' @ revision 3'"
        >
          {{ file }} · Revision 3
        </button>
      </details>
      <div class="wf-actions">
        <button class="btn btn-primary" @click="addResult">Add Result to Draft</button
        ><button class="btn" @click="router.push(wurl('/chat'))">Return to Chat</button>
      </div>
      <details class="mt-5">
        <summary>Identity, Budget & Unhandled Requests</summary>
        <p>Source Session A · fixture-run</p>
        <p>Model summary is an opinion. Your acceptance: Not Reviewed.</p>
        <p>Chat todos: 3 · Persistent task: task-17 · Blueprint run: fixture-run</p>
        <p>Usage unknown · Root ledger preserved · Unhandled request req-9 retained</p>
        <p>Real model calls: 0</p>
      </details>
    </section>
    <div v-else-if="mode === 'supervision'" class="wf-run-card">
      <p>{{ reason }} Requests are separate from approvals.</p>
      <div class="wf-actions">
        <button class="btn" @click="fixtureControl('Pause')">Pause Run</button
        ><button class="btn" @click="fixture.dialog = 'Stop Run'">Stop Run</button
        ><button class="btn" @click="router.push(wurl('/chat'))">Back to Original Chat</button>
      </div>
      <p v-if="fixture.state === 'Missing Model'">
        No supervision model configured. Direct controls remain available.
      </p>
    </div>
    <template v-else-if="props.mode === 'dialog'"
      ><h2>{{ fixture.dialog }}</h2>
      <template v-if="fixture.dialog === 'Compare Changes'"
        ><p>
          Local recovery draft, common baseline and latest disk. Confirmed content still uses a
          conditional write.
        </p>
        <div class="wf-compare">
          <div>
            <h3>Common Baseline</h3>
            <pre># Metrics\nOriginal text</pre>
          </div>
          <div>
            <h3>Your Draft</h3>
            <pre>{{ doc?.content }}</pre>
          </div>
          <div>
            <h3>Latest Disk · r{{ fixture.diskRevision }}</h3>
            <pre>{{ fixture.disk }}</pre>
          </div>
        </div>
        <div class="wf-actions">
          <button class="btn" @click="externalChange">Simulate External Change</button
          ><button class="btn" @click="reviewLatest">Review Latest</button
          ><button class="btn" @click="keep('both')">Keep Both</button
          ><button class="btn" @click="keep('disk')">Use Disk</button
          ><button class="btn btn-primary" @click="keep('save')">Save Reviewed Content</button>
        </div></template
      >
      <template v-else-if="fixture.dialog === 'Recovery Copies'"
        ><p>
          Recovery copies remain separate by window and source. Choose one explicitly; the saved
          file is unchanged.
        </p>
        <div class="wf-run-card">
          <strong>This Window</strong>
          <p>{{ fixture.backup ? 'Host-confirmed copy · Fixture' : 'No confirmed copy' }}</p>
          <button class="btn" @click="recover">Review Confirmed Copy</button>
        </div>
        <div class="wf-run-card">
          <strong>Window 2 · Independent Copy</strong>
          <p>Same path, separate draft identity · Fixture</p>
          <button
            class="btn"
            @click="fixture.notice = 'Other window copy retained; no automatic overwrite'"
          >
            Inspect Copy
          </button>
        </div>
        <div class="wf-run-card">
          <strong>Deleted Session · Orphaned Draft</strong>
          <p>Copy text manually without recreating the source session.</p>
          <button class="btn" @click="copyOrphan">Copy Text to Draft</button>
        </div></template
      ><template v-else-if="fixture.dialog === 'Reference File'"
        ><p>Unsaved edits exist. Choose the exact content to reference.</p>
        <div class="wf-actions">
          <button class="btn" @click="useSaved">Use Saved Version</button
          ><button class="btn" @click="compare">Save Then Reference</button>
        </div>
        <p>Later tool reads observe the current version at read time.</p></template
      >
      <template v-else-if="fixture.dialog === 'Discard Draft'"
        ><p>
          Discard this copy only? A tombstone rejects late backups; other windows retain independent
          copies. Orphaned drafts can be copied without recreating deleted sessions.
        </p>
        <button class="btn" @click="discardFixture">Discard This Copy</button></template
      >
      <template v-else-if="fixture.dialog === 'Review Launch'"
        ><label
          >Source
          <select v-model="fixture.source" class="input" aria-label="Launch Source">
            <option>Chat Handoff</option>
            <option>Editor Direct Run</option>
          </select></label
        >
        <p>
          {{
            fixture.source === 'Editor Direct Run'
              ? 'Independent run; no source conversation; no automatic continuation'
              : 'Session A through turn-6 · Blueprint r3 / exact digest'
          }}
        </p>
        <label v-if="fixture.source === 'Chat Handoff'"
          >After Completion
          <select v-model="fixture.policy" class="input">
            <option>Keep Results</option>
            <option>Continue Source Conversation</option>
          </select></label
        >
        <p>
          Personal / Repair r2 → Project / Repair r5 → Personal / Repair r2. Ordered immutable
          snapshots.
        </p>
        <button
          class="btn btn-primary"
          :disabled="fixture.state === 'Stale Version' || fixture.state === 'Disconnected'"
          @click="confirmLaunch"
        >
          Confirm This Version
        </button></template
      >
      <template v-else-if="fixture.dialog === 'Stop Run'"
        ><p>
          Stop fixture-run and its automatic continuation? Existing file changes and unhandled
          requests remain available. Stop does not restore prior files.
        </p>
        <button class="btn" @click="stopFixture">Stop This Run</button></template
      >
      <template v-else
        ><p>{{ reason }}</p>
        <p>
          Journal command-17 · Receipt effect-9 · Check the actual saved versions. No generic
          Continue is available for unknown effects, exhausted budget, revoked permissions or
          invalid sources.
        </p>
        <button class="btn" @click="fixture.notice = 'Fixture journal opened; nothing replayed'">
          View Journal
        </button></template
      >
      <div class="wf-actions"><button class="btn" @click="fixture.dialog = ''">Close</button></div>
      <details class="mt-4 text-muted-foreground">
        <summary>Preview Details</summary>
        <p>Isolated fixture r3 · No real action or authorization</p>
      </details>
    </template>
  </div>
</template>
