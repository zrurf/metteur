<script setup lang="ts">
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { gateway } from '@/core'
import { useCodingWorkflowsStore } from '@/stores/coding-workflows'
import { useWorkspaceStore } from '@/stores/workspace'
import { useRecoveryStore } from '@/stores/recovery'
import { useChatStore } from '@/stores/chat'
import { useComposerInputsStore } from '@/stores/composer-inputs'
import { useTabsStore } from '@/stores/tabs'
import { useSurfaceNavigation } from '@/lib/surface'
import { fileRoute } from '@/lib/file-token'
import { validResultReference, type ResultChoice } from '@/core/workflow'
import type { RecoveryRecord } from '@/core/recovery'
import WorkflowPlanner from './WorkflowPlanner.vue'
import WorkflowReview from './WorkflowReview.vue'
import PageHeader from '@/components/PageHeader.vue'
import RecoveryCopies from '@/components/RecoveryCopies.vue'
const workflows = useCodingWorkflowsStore(),
  workspace = useWorkspaceStore(),
  recovery = useRecoveryStore(),
  chat = useChatStore(),
  inputs = useComposerInputsStore(),
  tabs = useTabsStore(),
  router = useRouter(),
  { openSurface } = useSurfaceNavigation()
const root = computed(() => workspace.active?.path ?? ''),
  s = computed(() => workflows.current),
  copies = ref(false)
const backup = computed(() => {
  const p = recovery.status(root.value, 'workflow', s.value.plan.input.instance_id)
  return p?.state === 'backed_up' ? 'Plan backed up by the host' : p?.error || 'Plan backup pending'
})
function file(path: string) {
  tabs.openFile(path)
  void router.push(fileRoute(path))
}
async function back() {
  const source = s.value.plan.source
  s.value.mode = ''
  if (source?.session) {
    await chat.switchTo(source.session)
    if (chat.sessionId === source.session) openSurface('chat')
  }
}
function restored(record: RecoveryRecord) {
  workflows.restore(root.value, record)
  copies.value = false
}
async function prepareLaunch() {
  const path = root.value,
    plan = s.value.plan,
    source = plan.source
  if (!source?.session) {
    file(plan.saved)
    return
  }
  await chat.switchTo(source.session)
  if (root.value !== path || chat.sessionId !== source.session || chat.sessionUnavailable) {
    s.value.error = 'The original conversation is unavailable. The saved blueprint is retained.'
    return
  }
  const entry = inputs.read(path, source.entity)
  if (!entry.controller.canSubmit) {
    s.value.error = 'Reconcile the existing chat submission before adding a new proposal.'
    return
  }
  inputs.edit(path, source.entity, {
    raw: [
      entry.input.raw,
      `Propose running the saved blueprint ${plan.saved} (ID ${plan.input.instance_id}) for this goal: ${plan.input.goal}. Freeze its current version and dependencies using RequestBlueprintRun. Keep this original conversation and retain the result for my review; do not automatically continue. Preserve any existing task budget and history.`,
    ]
      .filter(Boolean)
      .join('\n\n'),
  })
  openSurface('chat')
}
async function followup() {
  const path = root.value,
    review = s.value.review
  if (!review) return
  if (review.source?.kind === 'unavailable') {
    s.value.error =
      'The original source could not be verified. Reconnect and refresh before preparing a follow-up.'
    return
  }
  const target = review.source?.kind === 'chat' ? review.source.session : chat.sessionId
  if (target && target !== chat.sessionId) await chat.switchTo(target)
  if (root.value !== path || (target && chat.sessionId !== target) || chat.sessionUnavailable)
    return
  const entity = chat.draftEntity,
    entry = inputs.read(path, entity)
  if (!entry.controller.canSubmit) {
    s.value.error = 'Reconcile the current chat submission before changing its draft.'
    return
  }
  const captured = await gateway.resourceRequest('queryContinuations', path, {
    operation: 'capture_result',
    reference: { kind: 'run', runId: review.run_id, digest: '' },
  })
  if (root.value !== path || chat.draftEntity !== entity) return
  if (!captured.ok) {
    s.value.error = captured.error
    return
  }
  const result = captured.data as ResultChoice
  if (!validResultReference(result.reference)) {
    s.value.error = 'The host did not confirm the result reference.'
    return
  }
  const unmet = review.criteria.filter(
    (c) => c.phase === 'acceptance' && (!c.decision?.current || c.decision.decision !== 'accepted'),
  )
  chat.queueResult({ label: review.goal, reference: result.reference })
  chat.draft = [
    chat.draft,
    `Prepare a new proposal for the saved result. Unmet requirements: ${unmet.map((c) => `${c.description} (${c.status})`).join('; ')}. Preserve the existing task history and root budget. Do not restart the old run.`,
  ]
    .filter(Boolean)
    .join('\n\n')
  openSurface('chat')
}
</script>
<template>
  <div class="workflow-workbench">
    <div class="workflow-context">
      <span>{{ s.mode === 'review' ? 'Run result' : 'Coding plan' }}</span
      ><button class="btn" @click="s.mode = ''">Back to tasks</button
      ><button class="btn" @click="copies = true">Recovery copies</button
      ><span v-if="s.mode === 'plan'">{{ backup }}</span>
    </div>
    <WorkflowPlanner
      v-if="s.mode === 'plan' && !s.plan.saved"
      v-model="s.plan.input"
      :catalog="s.catalog"
      :models="s.models"
      :busy="s.busy"
      :error="s.error"
      :prepared="!!s.plan.prepared"
      :warnings="s.plan.warnings"
      :backup="backup"
      @changed="workflows.changed(root)"
      @preview="workflows.preview(root)"
      @create="workflows.create(root)"
      @cancel="back"
    />
    <section v-else-if="s.mode === 'plan'" class="workflow-saved">
      <PageHeader
        :title="s.plan.input.name"
        description="Saved blueprint · execution has not started"
      />
      <p role="alert" v-if="s.error">{{ s.error }}</p>
      <p role="status">{{ s.notice }}</p>
      <p>{{ s.plan.saved }}</p>
      <p>{{ s.plan.input.goal }}</p>
      <div class="workflow-context">
        <button class="btn btn-primary" :disabled="s.busy || !!s.error" @click="prepareLaunch">
          {{ s.plan.source ? 'Prepare run in original Chat' : 'Open blueprint' }}</button
        ><button class="btn" @click="file(s.plan.saved)">Edit saved blueprint</button
        ><button
          v-if="s.error && s.plan.prepared"
          class="btn"
          :disabled="s.busy"
          @click="workflows.create(root)"
        >
          Retry synchronization
        </button>
      </div>
      <p v-if="s.plan.source">
        The proposal is added to the original draft. Send explicitly to request the existing version
        and handoff confirmation.
      </p>
      <p v-else>This is an independent blueprint. Its ordinary editor owns save and run.</p>
    </section>
    <template v-else-if="s.mode === 'review'"
      ><div v-if="s.pending" class="workflow-context" role="status">
        <span>A review request needs reconciliation.</span
        ><button class="btn" :disabled="s.busy" @click="workflows.reconcile(root)">
          Check original receipt
        </button>
      </div>
      <WorkflowReview
        v-if="s.review"
        :key="s.run"
        :review="s.review"
        :saved-draft="s.draft"
        :error="s.error"
        :notice="s.notice"
        :busy="s.busy || !!s.pending"
        @draft="(criterion, note) => workflows.editReview(root, criterion, note)"
        @decide="(value) => workflows.decide(root, value)"
        @refresh="workflows.refresh(root)"
        @file="file"
        @prepare="followup"
      />
      <div v-else class="workflow-saved" role="status">
        <p>
          {{
            s.busy
              ? 'Reading the recorded requirements…'
              : s.error || 'No acceptance criteria available.'
          }}
        </p>
        <button class="btn" :disabled="s.busy" @click="workflows.refresh(root)">
          Refresh evidence
        </button>
      </div></template
    >
    <RecoveryCopies
      v-if="copies"
      :workspace="root"
      scope="project"
      @close="copies = false"
      @restore="restored"
    />
  </div>
</template>
<style scoped>
.workflow-workbench {
  height: 100%;
  overflow: auto;
  font-size: 14px;
  line-height: 1.6;
}
.workflow-context {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 12px;
  padding: 12px 24px;
  font-size: 13px;
  border-bottom: 1px solid var(--divider);
}
.workflow-saved {
  display: grid;
  gap: 18px;
  max-width: 1040px;
  margin: auto;
  padding: 24px 28px;
}
.workflow-saved :deep(.surface-header) {
  padding: 0;
}
.workflow-saved .workflow-context {
  padding: 0;
  border: 0;
}
</style>
