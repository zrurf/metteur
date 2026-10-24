import type { TemplateInfo, TemplateInput, WorkflowReview } from '@/core/coding-workflows'
export const catalog: TemplateInfo[] = [
  {
    id: 'fix_bug',
    revision: 1,
    title: 'Fix a bug',
    description: 'Reproduce a known failure, make a focused fix and verify the result.',
    acceptance: 'The frozen regression fails before the change and passes afterwards.',
  },
  {
    id: 'add_tests',
    revision: 1,
    title: 'Add tests',
    description: 'Capture existing behavior and prove the tests detect a specified defect.',
    acceptance: 'New tests pass on correct code and detect the specified defect.',
  },
  {
    id: 'refactor',
    revision: 1,
    title: 'Refactor safely',
    description: 'Improve implementation while preserving observable behavior.',
    acceptance: 'Independent behavior and public interface checks remain unchanged.',
  },
  {
    id: 'add_feature',
    revision: 1,
    title: 'Add a small feature',
    description: 'Deliver a bounded feature with normal, edge and failure cases.',
    acceptance: 'New acceptance cases and existing regression checks pass.',
  },
]
export function exampleInput(): TemplateInput {
  return {
    instance_id: '478e3c4b-613a-49f3-b181-9d5b20e359ad',
    template: 'fix_bug',
    name: 'Handle empty metric samples',
    goal: 'Return an empty result for an empty sample list while preserving normalization for nonempty input.',
    constraints: 'Keep the normalize signature and both existing callers unchanged.',
    scope_paths: ['metrics/normalize.py'],
    evidence_paths: ['metrics/normalize.py', 'tests/test_normalize.py'],
    model: 'Fixture model · no calls',
    max_iterations: 8,
    baseline: {
      id: 'baseline',
      description: 'Reproduce the empty-input regression',
      command: 'python -m unittest tests.test_normalize.EmptyInput',
      expected_exit_code: 1,
      required: true,
    },
    checks: [
      {
        id: 'regression',
        description: 'Empty input works and existing behavior is preserved',
        command: 'python -m unittest discover -s tests',
        expected_exit_code: 0,
        required: true,
      },
    ],
    manual_checks: [
      { id: 'scope', description: 'Changes stay within the agreed scope', required: true },
    ],
  }
}
export function exampleReview(): WorkflowReview {
  return {
    run_id: '08f5b536-7a0d-4678-9b54-72a20d922a2b',
    goal: 'Handle empty metric samples',
    status: 'Completed',
    digest: 'fixture-evidence-r1',
    revision: 0,
    can_review: true,
    coverage:
      'Fixture evidence. Only the selected source and test files are represented; external effects are not covered.',
    criteria: [
      {
        id: 'baseline',
        description: 'Reproduce the empty-input regression',
        required: true,
        phase: 'baseline',
        status: 'passed',
        reason: 'The frozen baseline returned the expected exit code 1 before implementation.',
        command: 'python -m unittest tests.test_normalize.EmptyInput',
        output: 'FAILED (errors=1)\nExpected baseline failure observed.',
        files: [{ path: 'metrics/normalize.py', status: 'Recorded baseline' }],
      },
      {
        id: 'regression',
        description: 'Empty input works and existing behavior is preserved',
        required: true,
        phase: 'acceptance',
        status: 'passed',
        reason:
          'The independent regression command returned exit code 0. Evidence matches the reviewed files.',
        command: 'python -m unittest discover -s tests',
        output: 'Ran 8 tests\nOK\nExit code: 0',
        files: [
          { path: 'metrics/normalize.py', status: 'Matches checked version' },
          { path: 'tests/test_normalize.py', status: 'Matches checked version' },
        ],
      },
      {
        id: 'scope',
        description: 'Changes stay within the agreed scope',
        required: true,
        phase: 'acceptance',
        status: 'needs_review',
        reason: 'Review the file changes before accepting this requirement.',
        files: [{ path: 'metrics/normalize.py', status: 'Ready to review' }],
      },
    ],
  }
}
