import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  matchesReviewReceipt,
  isTemplateDraft,
  isPendingReview,
} from '../src/core/coding-workflows.ts'

test('exhausted review revision remains a recoverable pending request', () => {
  const run = crypto.randomUUID(),
    pending = {
      request: crypto.randomUUID(),
      expected: 64,
      input: {
        run_id: run,
        criterion_id: 'scope',
        decision: 'changes_requested',
        note: 'Final review',
        evidence_digest: 'a'.repeat(64),
      },
    }
  assert.equal(isPendingReview(pending, run), true)
  assert.equal(isPendingReview({ ...pending, expected: 65 }, run), false)
  assert.equal(isPendingReview(pending, crypto.randomUUID()), false)
})

test('only the durable acknowledgement for this exact decision clears an uncertain request', () => {
  const pending = {
    request: crypto.randomUUID(),
    expected: 2,
    input: {
      run_id: crypto.randomUUID(),
      criterion_id: 'scope',
      decision: 'accepted',
      note: 'Reviewed',
      evidence_digest: 'a'.repeat(64),
    },
  }
  const receipt = {
    request_id: pending.request,
    receipt: {
      schema_version: 1,
      revision: 3,
      value: {
        run_id: pending.input.run_id,
        subject: 'local',
        decisions: { scope: { ...pending.input, subject: 'local' } },
      },
    },
  }
  assert.equal(matchesReviewReceipt(receipt, pending), true)
  for (const mutate of [
    (r) => {
      r.request_id = crypto.randomUUID()
    },
    (r) => {
      r.receipt.revision = 4
    },
    (r) => {
      r.receipt.value.run_id = crypto.randomUUID()
    },
    (r) => {
      r.receipt.value.decisions.scope.note = 'Another decision'
    },
    (r) => {
      r.receipt.value.decisions.scope.evidence_digest = 'b'.repeat(64)
    },
    (r) => {
      r.receipt.value.decisions.scope.subject = 'other-user'
    },
  ]) {
    const wrong = structuredClone(receipt)
    mutate(wrong)
    assert.equal(matchesReviewReceipt(wrong, pending), false)
  }
  for (const malformed of [null, {}, { receipt: {} }, 'success'])
    assert.equal(matchesReviewReceipt(malformed, pending), false)
})

test('partial form content survives recovery but malformed nested fields do not mount', () => {
  const draft = {
    instance_id: crypto.randomUUID(),
    template: 'fix_bug',
    name: '',
    goal: '',
    constraints: '',
    model: '',
    scope_paths: [],
    evidence_paths: [],
    max_iterations: 8,
    baseline: {
      id: 'baseline',
      description: '',
      command: '',
      expected_exit_code: 1,
      required: true,
    },
    checks: [],
    manual_checks: [],
  }
  assert.equal(isTemplateDraft(draft), true)
  for (const patch of [
    { baseline: null },
    { checks: [null] },
    { scope_paths: [3] },
    { manual_checks: [{}] },
    { template: 'future' },
  ])
    assert.equal(isTemplateDraft({ ...draft, ...patch }), false)
})
