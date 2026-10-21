<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { vDialogFocus } from '@/components/resources/dialog-focus'
import './continuations.css'
const props = defineProps<{ models: string[]; busy?: boolean; error?: string }>()
const emit = defineEmits<{
  close: []
  start: [
    {
      goal: string
      model: string
      maxTurns: number
      tokenLimit: number
      minutes: number
      detached: boolean
    },
  ]
}>()
const goal = ref(''),
  model = ref(props.models[0] ?? ''),
  maxTurns = ref(4),
  tokenLimit = ref(200000),
  minutes = ref(60),
  detached = ref(true),
  consent = ref(false)
watch([goal, model, maxTurns, tokenLimit, minutes, detached], () => {
  consent.value = false
})
const valid = computed(
  () =>
    goal.value.trim().length > 0 &&
    goal.value.length <= 2048 &&
    model.value &&
    maxTurns.value >= 1 &&
    maxTurns.value <= 20 &&
    tokenLimit.value >= 1 &&
    tokenLimit.value <= 10000000 &&
    minutes.value >= 1 &&
    minutes.value <= 10080 &&
    consent.value,
)
function submit() {
  if (valid.value && !props.busy)
    emit('start', {
      goal: goal.value.trim(),
      model: model.value,
      maxTurns: maxTurns.value,
      tokenLimit: tokenLimit.value,
      minutes: minutes.value,
      detached: detached.value,
    })
}
</script>
<template>
  <div class="nx-live">
    <div
      class="nx-live-overlay"
      @click.self="!busy && emit('close')"
      @keydown.esc="!busy && emit('close')"
    >
      <section
        v-dialog-focus
        class="nx-live-modal"
        role="dialog"
        aria-modal="true"
        aria-label="Start a continuation task"
      >
        <div class="nx-live-between">
          <h2>Start a continuation task</h2>
          <button class="btn" :disabled="busy" @click="emit('close')">Close</button>
        </div>
        <label style="display: grid; gap: 8px; font-size: 13px"
          >Goal<textarea
            v-model="goal"
            class="input"
            rows="3"
            maxlength="2048"
            placeholder="Describe the task and what to do after its results arrive"
          />
        </label>
        <div class="nx-live-form" style="margin-top: 18px">
          <label
            >Configured model<select v-model="model" class="input">
              <option v-for="m in models" :key="m" :value="m">{{ m }}</option>
            </select></label
          ><label
            >Maximum turns, including the first<input
              v-model.number="maxTurns"
              type="number"
              min="1"
              max="20"
              class="input" /></label
          ><label
            >Total token limit<input
              v-model.number="tokenLimit"
              type="number"
              min="1"
              max="10000000"
              class="input" /></label
          ><label
            >Expires after minutes<input
              v-model.number="minutes"
              type="number"
              min="1"
              max="10080"
              class="input"
          /></label>
        </div>
        <label class="nx-live-consent"
          ><input v-model="detached" type="checkbox" />Allow this task to continue when this browser
          disconnects</label
        >
        <div class="nx-live-alert">
          <div>
            <strong>A new dedicated task session</strong>
            <p>
              The first turn starts immediately. Later turns may resume only for this task’s owned
              results and saved plan. Existing tool permissions still apply; no new write
              permissions are granted.
            </p>
            <p>
              Usage is shared across parent and child calls. Unknown usage blocks further automatic
              calls. No currency estimate is available.
            </p>
          </div>
        </div>
        <label class="nx-live-consent"
          ><input v-model="consent" type="checkbox" />Authorize this exact goal, model, time and
          limits</label
        >
        <p v-if="error" role="alert" class="nx-live-alert">{{ error }}</p>
        <div class="nx-live-actions">
          <button class="btn" :disabled="busy" @click="emit('close')">Cancel</button
          ><button class="btn btn-primary" :disabled="!valid || busy" @click="submit">
            {{ busy ? 'Starting…' : 'Start task' }}
          </button>
        </div>
      </section>
    </div>
  </div>
</template>
