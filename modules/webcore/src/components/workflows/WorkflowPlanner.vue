<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { TemplateInfo, TemplateInput, TemplateKind } from '@/core/coding-workflows'
import { plannerIssues } from '@/core/coding-workflows'
import PageHeader from '@/components/PageHeader.vue'
const input = defineModel<TemplateInput>({ required: true })
const props = defineProps<{
  catalog: TemplateInfo[]
  models: string[]
  busy?: boolean
  error?: string
  prepared?: boolean
  backup?: string
  warnings?: string[]
}>()
const emit = defineEmits<{ preview: []; create: []; cancel: []; changed: [] }>()
const step = ref<'choose' | 'configure' | 'plan'>('choose')
const selected = computed(() => props.catalog.find((t) => t.id === input.value.template))
const issues = computed(() => plannerIssues(input.value))
watch(
  () => props.prepared,
  (value) => {
    if (value) step.value = 'plan'
  },
)
function select(kind: TemplateKind) {
  input.value.template = kind
  input.value.baseline.expected_exit_code = kind === 'fix_bug' ? 1 : 0
  step.value = 'configure'
  emit('changed')
}
function paths(event: Event, key: 'scope_paths' | 'evidence_paths') {
  input.value[key] = (event.target as HTMLTextAreaElement).value
    .split('\n')
    .map((s) => s.trim())
    .filter(Boolean)
  emit('changed')
}
function addCheck() {
  input.value.checks.push({
    id: crypto.randomUUID(),
    description: '',
    command: '',
    expected_exit_code: 0,
    required: true,
  })
  emit('changed')
}
function addReview() {
  input.value.manual_checks.push({ id: crypto.randomUUID(), description: '', required: true })
  emit('changed')
}
function removeCheck(index: number, manual = false) {
  if (manual) input.value.manual_checks.splice(index, 1)
  else input.value.checks.splice(index, 1)
  emit('changed')
}
</script>
<template>
  <section
    class="coding-workflow"
    aria-label="Coding workflow planner"
    :inert="busy"
    :aria-busy="busy"
  >
    <PageHeader
      title="Start from template"
      description="Agree on the goal, scope and acceptance before running."
      ><button class="btn" @click="emit('cancel')">Back to source</button></PageHeader
    >
    <nav class="workflow-steps" aria-label="Plan steps">
      <button
        class="btn"
        :aria-current="step === 'choose' ? 'step' : undefined"
        @click="step = 'choose'"
      >
        1 · Template</button
      ><button
        class="btn"
        :aria-current="step === 'configure' ? 'step' : undefined"
        @click="step = 'configure'"
      >
        2 · Goal &amp; checks</button
      ><button
        class="btn"
        :disabled="!prepared"
        :aria-current="step === 'plan' ? 'step' : undefined"
        @click="step = 'plan'"
      >
        3 · Review plan
      </button>
    </nav>
    <p v-if="error" role="alert" class="workflow-error">{{ error }}</p>
    <details v-if="prepared && warnings?.length" class="workflow-muted">
      <summary>{{ warnings.length }} declared files are not currently available</summary>
      <ul>
        <li v-for="warning in warnings" :key="warning">{{ warning }}</li>
      </ul>
      <p>
        New output files may not exist yet. Commands and external dependencies are verified during
        execution.
      </p>
    </details>
    <div v-if="step === 'choose'" class="workflow-template-grid">
      <button
        v-for="item in catalog"
        :key="item.id"
        class="workflow-template"
        @click="select(item.id)"
      >
        <strong>{{ item.title }}</strong
        ><span>{{ item.description }}</span
        ><span class="workflow-muted">{{ item.acceptance }}</span>
      </button>
    </div>
    <form
      v-else-if="step === 'configure'"
      class="workflow-form"
      @input="emit('changed')"
      @submit.prevent="emit('preview')"
    >
      <h2>{{ selected?.title }}</h2>
      <fieldset>
        <legend>Goal</legend>
        <label>Plan name<input v-model="input.name" class="input" maxlength="120" required /></label
        ><label
          >Expected outcome<textarea
            v-model="input.goal"
            class="input"
            rows="2"
            maxlength="4096"
            required
          />
        </label>
      </fieldset>
      <fieldset>
        <legend>Scope</legend>
        <label
          >Files in scope<textarea
            class="input mono"
            :value="input.scope_paths.join('\n')"
            rows="2"
            placeholder="One workspace-relative file per line"
            @input="paths($event, 'scope_paths')"
          />
        </label>
        <details>
          <summary>Constraints &amp; evidence files</summary>
          <div class="workflow-form">
            <label
              >Keep unchanged<textarea
                v-model="input.constraints"
                class="input"
                rows="2"
                maxlength="4096"
              /></label
            ><label
              >Evidence files<textarea
                class="input mono"
                :value="input.evidence_paths.join('\n')"
                rows="2"
                placeholder="Include source, tests and relevant configuration"
                @input="paths($event, 'evidence_paths')"
              />
            </label>
            <p class="workflow-muted">
              Only these declared files are compared with the checked version. Scope does not expand
              execution permissions.
            </p>
          </div>
        </details>
      </fieldset>
      <fieldset>
        <legend>Acceptance</legend>
        <div v-for="(check, index) in input.checks" :key="check.id" class="workflow-check-editor">
          <label
            >Requirement {{ index + 1
            }}<input v-model="check.description" class="input" maxlength="1024" required
          /></label>
          <details>
            <summary>Check command &amp; requirement options</summary>
            <div class="workflow-form">
              <label
                >Check command<input v-model="check.command" class="input mono" maxlength="4096"
              /></label>
              <div class="workflow-actions">
                <label class="workflow-inline"
                  ><input v-model="check.required" type="checkbox" />Required</label
                ><button
                  v-if="input.checks.length > 1"
                  class="btn"
                  type="button"
                  @click="removeCheck(index)"
                >
                  Remove check
                </button>
              </div>
              <p class="workflow-muted">
                Passes only when the recorded command returns exit code 0 and evidence is current.
              </p>
            </div>
          </details>
        </div>
        <div
          v-for="(check, index) in input.manual_checks"
          :key="check.id"
          class="workflow-check-editor"
        >
          <label
            >Human review {{ index + 1
            }}<input v-model="check.description" class="input" maxlength="1024" required
          /></label>
          <details>
            <summary>Review options</summary>
            <div class="workflow-actions">
              <label class="workflow-inline"
                ><input v-model="check.required" type="checkbox" />Required</label
              ><button class="btn" type="button" @click="removeCheck(index, true)">
                Remove review
              </button>
            </div>
          </details>
        </div>
        <div class="workflow-actions">
          <button
            class="btn"
            type="button"
            :disabled="input.checks.length + input.manual_checks.length >= 15"
            @click="addCheck"
          >
            Add check</button
          ><button
            class="btn"
            type="button"
            :disabled="input.checks.length + input.manual_checks.length >= 15"
            @click="addReview"
          >
            Add human review
          </button>
        </div>
      </fieldset>
      <details class="workflow-settings">
        <summary>Baseline &amp; execution settings · {{ input.model || 'select a model' }}</summary>
        <div class="workflow-form">
          <label
            >Baseline command<input
              v-model="input.baseline.command"
              class="input mono"
              maxlength="4096" /></label
          ><label
            >Expected baseline exit code<input
              v-model.number="input.baseline.expected_exit_code"
              class="input"
              type="number"
              min="0"
              max="255"
          /></label>
          <p class="workflow-muted">
            The baseline must match before implementation begins. A timeout is not an expected
            failure.
          </p>
          <div class="workflow-columns">
            <label
              >Model<select v-model="input.model" class="input">
                <option value="">Select a configured model</option>
                <option v-for="model in models" :key="model">{{ model }}</option>
              </select></label
            ><label
              >Maximum iterations<input
                v-model.number="input.max_iterations"
                type="number"
                class="input"
                min="1"
                max="64"
            /></label>
          </div>
        </div>
      </details>
      <ul v-if="issues.length" class="workflow-muted">
        <li v-for="issue in issues" :key="issue">{{ issue }}</li>
      </ul>
      <footer class="workflow-actions">
        <button class="btn btn-primary" type="submit" :disabled="busy || !!issues.length">
          {{ busy ? 'Preparing…' : 'Preview plan' }}</button
        ><span role="status" class="workflow-muted">{{
          backup || 'No commands or model calls during preview.'
        }}</span>
      </footer>
    </form>
    <div v-else class="workflow-form">
      <h2>{{ input.name }}</h2>
      <p>{{ input.goal }}</p>
      <ol class="workflow-stage-list">
        <li>
          <strong>Verify the baseline</strong>
          <p>Run the baseline and require exit code {{ input.baseline.expected_exit_code }}.</p>
        </li>
        <li>
          <strong>Implement the scoped change</strong>
          <p>Use {{ input.model }} within the chosen scope and existing permissions.</p>
        </li>
        <li>
          <strong>Check and review the result</strong>
          <p>Run {{ input.checks.length }} acceptance check(s), then review the evidence.</p>
        </li>
      </ol>
      <div class="workflow-callout">
        <strong>Creating a blueprint does not start it.</strong>
        <p>
          Review the saved version before running. The original conversation stays in Chat;
          supervision opens separately.
        </p>
      </div>
      <details>
        <summary>Commands, limits &amp; node details</summary>
        <p>{{ input.constraints }}</p>
        <p>
          Model iterations: {{ input.max_iterations }}. Checks run once by default. No automatic
          rollback or retry.
        </p>
        <ul>
          <li>
            <code>{{ input.baseline.command }}</code> · expected
            {{ input.baseline.expected_exit_code }}
          </li>
          <li v-for="check in input.checks" :key="check.id">
            <code>{{ check.command }}</code> · expected 0
          </li>
        </ul>
        <p>
          Start → baseline command → exit-code validator → conditional implementation → acceptance
          commands and validators → End.
        </p>
        <p>
          File scope expresses task intent. Existing sandbox and command approvals remain in force.
        </p>
      </details>
      <footer class="workflow-actions">
        <button class="btn" @click="step = 'configure'">Edit plan</button
        ><button class="btn btn-primary" :disabled="busy || !prepared" @click="emit('create')">
          Create blueprint
        </button>
      </footer>
    </div>
  </section>
</template>
<style scoped src="./workflows.css"></style>
