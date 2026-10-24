<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { CriterionResult, ReviewDecision, WorkflowReview } from '@/core/coding-workflows'
import PageHeader from '@/components/PageHeader.vue'
const props = defineProps<{
  review: WorkflowReview
  busy?: boolean
  error?: string
  notice?: string
  savedDraft?: ReviewDecision | null
}>()
const emit = defineEmits<{
  decide: [decision: ReviewDecision]
  draft: [criterion: string, note: string]
  prepare: []
  file: [path: string]
  refresh: []
}>()
const editing = ref(''),
  note = ref('')
watch(
  () => props.review.run_id,
  () => {
    editing.value = props.savedDraft?.criterion_id ?? ''
    note.value = props.savedDraft?.note ?? ''
  },
  { immediate: true },
)
watch([editing, note], () => emit('draft', editing.value, note.value))
const required = computed(() =>
  props.review.criteria.filter((c) => c.required && c.phase === 'acceptance'),
)
const checks = computed(() => required.value.filter((c) => !!c.command))
const passed = computed(() => checks.value.filter((c) => c.status === 'passed').length)
const accepted = computed(
  () =>
    required.value.filter((c) => c.decision?.current && c.decision.decision === 'accepted').length,
)
const baseline = computed(() => props.review.criteria.filter((c) => c.phase === 'baseline'))
const delivery = computed(() => props.review.criteria.filter((c) => c.phase === 'acceptance'))
const statuses = {
  passed: 'Passed',
  failed: 'Failed',
  missing: 'Missing',
  skipped: 'Skipped',
  stale: 'Stale',
  needs_review: 'Needs review',
}
function mayAccept(c: CriterionResult) {
  return props.review.can_review && ['passed', 'needs_review'].includes(c.status)
}
function decide(criterion_id: string, decision: ReviewDecision['decision']) {
  emit('decide', { criterion_id, decision, note: note.value })
}
</script>
<template>
  <section class="coding-workflow" aria-label="Workflow acceptance review">
    <PageHeader title="Review delivery" :description="review.goal"
      ><button class="btn" :disabled="busy" @click="emit('refresh')">
        Refresh evidence
      </button></PageHeader
    >
    <div class="workflow-summary">
      <span
        >Execution: <strong>{{ review.status }}</strong></span
      ><span
        >Checks: <strong>{{ passed }}/{{ checks.length }}</strong> passed</span
      ><span
        >Acceptance: <strong>{{ accepted }}/{{ required.length }}</strong> recorded</span
      >
    </div>
    <p v-if="error" role="alert" class="workflow-error">{{ error }}</p>
    <p v-if="notice" role="status" class="workflow-callout">{{ notice }}</p>
    <details v-if="baseline.length" class="workflow-baseline">
      <summary>
        Baseline ·
        {{
          baseline.every((c) => c.status === 'passed')
            ? 'matched before implementation'
            : 'needs attention'
        }}
      </summary>
      <div v-for="criterion in baseline" :key="criterion.id">
        <strong>{{ criterion.description }} · {{ statuses[criterion.status] }}</strong>
        <p>{{ criterion.reason }}</p>
        <code>{{ criterion.command }}</code>
        <pre v-if="criterion.output">{{ criterion.output }}</pre>
      </div>
    </details>
    <article v-for="criterion in delivery" :key="criterion.id" class="workflow-criterion">
      <header>
        <div>
          <h2>{{ criterion.description }}</h2>
          <p class="workflow-muted">
            {{
              criterion.phase === 'baseline'
                ? 'Baseline evidence'
                : criterion.required
                  ? 'Required'
                  : 'Optional'
            }}
          </p>
        </div>
        <span class="workflow-status" :data-status="criterion.status">{{
          statuses[criterion.status]
        }}</span>
      </header>
      <p>{{ criterion.reason }}</p>
      <details>
        <summary>View evidence</summary>
        <p v-if="criterion.command">
          <code>{{ criterion.command }}</code>
        </p>
        <pre v-if="criterion.output">{{ criterion.output }}</pre>
        <ul>
          <li v-for="file in criterion.files" :key="file.path">
            <button class="workflow-file" @click="emit('file', file.path)">{{ file.path }}</button>
            · {{ file.status }}
          </li>
        </ul>
      </details>
      <p v-if="criterion.decision" class="workflow-muted">
        {{ criterion.decision.current ? 'Recorded review' : 'Previous review · needs recheck' }}:
        {{ criterion.decision.decision === 'accepted' ? 'Accepted' : 'Changes requested' }} ·
        {{ criterion.decision.subject
        }}<span v-if="criterion.decision.note"> — {{ criterion.decision.note }}</span>
      </p>
      <div v-if="editing === criterion.id" class="workflow-review-editor">
        <label
          >Review note<textarea
            v-model="note"
            class="input"
            rows="2"
            maxlength="2048"
            placeholder="Explain any change needed"
          />
        </label>
        <div class="workflow-actions">
          <button
            class="btn btn-primary"
            :disabled="busy || !mayAccept(criterion)"
            @click="decide(criterion.id, 'accepted')"
          >
            Accept this requirement</button
          ><button
            class="btn"
            :disabled="busy || !review.can_review"
            @click="decide(criterion.id, 'changes_requested')"
          >
            Request changes</button
          ><button class="btn" :disabled="busy" @click="editing = ''">Cancel</button>
        </div>
      </div>
      <button
        v-else
        class="btn"
        :disabled="!review.can_review"
        @click="
          () => {
            editing = criterion.id
            note = criterion.decision?.note || ''
          }
        "
      >
        Review requirement
      </button>
    </article>
    <footer class="workflow-form">
      <p class="workflow-muted">{{ review.coverage }}</p>
      <p class="workflow-muted">
        Changing relevant files makes their previous evidence stale. Reviewing a result never reruns
        it.
      </p>
      <div class="workflow-actions">
        <button class="btn btn-primary" @click="emit('prepare')">Prepare follow-up in Chat</button
        ><span class="workflow-muted">Adds to the draft. Send when ready.</span>
      </div>
      <details>
        <summary>Run &amp; evidence identity</summary>
        <p class="mono">{{ review.run_id }}</p>
        <p class="mono">{{ review.digest }}</p>
        <p>Review revision {{ review.revision }}</p>
      </details>
    </footer>
  </section>
</template>
<style scoped src="./workflows.css"></style>
