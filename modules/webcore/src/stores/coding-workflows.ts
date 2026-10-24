import { computed, reactive } from 'vue'
import { defineStore } from 'pinia'
import { gateway } from '@/core'
import { nativeBlueprint, type NativeGraph } from '@/core/execution-view'
import type {
  TemplateInfo,
  TemplateInput,
  WorkflowReview,
  ReviewDecision,
} from '@/core/coding-workflows'
import type { RecoveryRecord } from '@/core/recovery'
import {
  matchesReviewReceipt,
  isTemplateDraft,
  isPendingReview,
  isReviewDecision,
} from '@/core/coding-workflows'
import { useRecoveryStore, type RecoveryMetadata } from './recovery'
import { useWorkspaceStore } from './workspace'

interface PendingReview {
  request: string
  expected: number
  input: ReviewDecision & { run_id: string; evidence_digest: string }
}
interface Plan {
  input: TemplateInput
  source: { session: string; entity: string } | null
  saved: string
  prepared?: NativeGraph
  fingerprint: string
  warnings: string[]
}
interface Session {
  mode: 'plan' | 'review' | ''
  plan: Plan
  review: WorkflowReview | null
  run: string
  pending: PendingReview | null
  draft: ReviewDecision | null
  reviewDrafts: Record<string, { pending: PendingReview | null; draft: ReviewDecision | null }>
  catalog: TemplateInfo[]
  models: string[]
  busy: boolean
  error: string
  notice: string
}
const clone = <T>(value: T): T => JSON.parse(JSON.stringify(value))
function initial(): Plan {
  return {
    source: null,
    saved: '',
    fingerprint: '',
    warnings: [],
    input: {
      instance_id: crypto.randomUUID(),
      template: 'fix_bug',
      name: '',
      goal: '',
      constraints: '',
      scope_paths: [],
      evidence_paths: [],
      model: '',
      max_iterations: 8,
      baseline: {
        id: 'baseline',
        description: 'Verify the baseline',
        command: '',
        expected_exit_code: 1,
        required: true,
      },
      checks: [
        { id: 'acceptance', description: '', command: '', expected_exit_code: 0, required: true },
      ],
      manual_checks: [{ id: 'scope', description: 'Review the scoped changes', required: true }],
    },
  }
}
function describe(raw: string) {
  try {
    return String(JSON.parse(raw).message ?? raw)
  } catch {
    return raw
  }
}
export const useCodingWorkflowsStore = defineStore('coding-workflows', () => {
  const workspace = useWorkspaceStore(),
    recovery = useRecoveryStore(),
    sessions = reactive(new Map<string, Session>())
  function session(root: string): Session {
    if (!sessions.has(root))
      sessions.set(root, {
        mode: '',
        plan: initial(),
        review: null,
        run: '',
        pending: null,
        draft: null,
        reviewDrafts: {},
        catalog: [],
        models: [],
        busy: false,
        error: '',
        notice: '',
      })
    return sessions.get(root)!
  }
  const current = computed(() => session(workspace.active?.path ?? ''))
  function backupPlan(root: string) {
    const s = session(root)
    recovery.edit(root, 'workflow', s.plan.input.instance_id, {
      raw: JSON.stringify({ input: s.plan.input, source: s.plan.source, saved: s.plan.saved }),
      selected_model: s.plan.input.model,
    })
  }
  function changed(root: string) {
    const s = session(root)
    s.plan.prepared = undefined
    s.plan.fingerprint = ''
    s.plan.warnings = []
    backupPlan(root)
  }
  async function openPlan(root: string, source: Plan['source'] = null, goal = '') {
    const s = session(root)
    if (s.busy) return
    if (s.plan.saved) {
      s.plan = initial()
    }
    if (!s.plan.source) s.plan.source = source
    if (!s.plan.input.goal) s.plan.input.goal = goal
    s.mode = 'plan'
    s.error = ''
    await catalog(root)
  }
  async function catalog(root: string) {
    const s = session(root)
    const r = await gateway.resourceRequest('queryCodingWorkflows', root, { operation: 'catalog' })
    if (!r.ok) {
      s.error = describe(r.error)
      return
    }
    const data = r.data as { templates: TemplateInfo[]; models: string[] }
    s.catalog = data.templates
    s.models = data.models
    if (!s.plan.input.model && s.models.length === 1) s.plan.input.model = s.models[0]!
  }
  async function preview(root: string) {
    const s = session(root)
    if (s.busy) return
    s.busy = true
    s.error = ''
    const input = clone(s.plan.input),
      key = JSON.stringify(input)
    try {
      const r = await gateway.resourceRequest('queryCodingWorkflows', root, {
        operation: 'compile',
        input,
      })
      if (!r.ok) {
        s.error = describe(r.error)
        return
      }
      if (key !== JSON.stringify(s.plan.input)) {
        s.error = 'The plan changed while preparing. Preview the current plan again.'
        return
      }
      s.plan.prepared = (r.data as { graph: NativeGraph }).graph
      s.plan.warnings = (
        (r.data as { preflight?: { files?: { path: string; status: string }[] } }).preflight
          ?.files ?? []
      )
        .filter((file) => file.status !== 'available')
        .map((file) => `${file.path}: ${file.status}`)
      s.plan.fingerprint = key
      backupPlan(root)
    } finally {
      s.busy = false
    }
  }
  async function create(root: string) {
    const s = session(root)
    if (s.busy || !s.plan.prepared || s.plan.fingerprint !== JSON.stringify(s.plan.input)) return
    s.busy = true
    s.error = ''
    const graph = clone(s.plan.prepared),
      body = JSON.stringify(graph, null, 2),
      name = s.plan.input.name
        .trim()
        .replace(/[<>:"/\\|?*]/g, '-')
        .split('')
        .map((c) => (c.charCodeAt(0) < 32 ? '-' : c))
        .join('')
        .replace(/[. ]+$/, '')
    const path = `${name || 'workflow'}.blueprint`
    try {
      backupPlan(root)
      if (!(await recovery.flush(root, 'workflow', s.plan.input.instance_id))) {
        s.error = 'Back up the plan before creating its file. Your input is retained.'
        return
      }
      const written = await gateway.writeFile(root, path, body, { createOnly: true })
      if (!written.ok) {
        const existing = await gateway.readFile(root, path)
        if (!existing.ok || existing.data.content !== body) {
          s.error = `Blueprint was not created: ${describe(written.error)}. Choose a different plan name if that file exists.`
          return
        }
      }
      const mirrored = await gateway.saveBlueprint(root, nativeBlueprint(graph), path)
      s.plan.saved = path
      backupPlan(root)
      if (!mirrored.ok) {
        s.error = `File saved, but its daemon mirror is unavailable: ${describe(mirrored.error)}. Retry synchronization before running.`
        return
      }
      s.notice = 'Blueprint saved. Review its saved version before running.'
      s.plan.prepared = undefined
    } finally {
      s.busy = false
    }
  }
  async function openReview(root: string, run: string) {
    const s = session(root)
    if (s.busy) return
    s.mode = 'review'
    s.run = run
    s.review = null
    s.pending = s.reviewDrafts[run]?.pending ?? null
    s.draft = s.reviewDrafts[run]?.draft ?? null
    s.error = ''
    s.notice = ''
    await refresh(root)
  }
  async function recoverOutstandingReview(root: string, run: string) {
    const s = session(root)
    s.busy = true
    try {
      let after: string | null = null
      const seen = new Set<string>()
      do {
        const listed = await recovery.list(root, 'project', after)
        if (!listed.ok) throw new Error(describe(listed.error))
        const page = listed.data as { items: RecoveryMetadata[]; next: string | null }
        for (const row of page.items.filter(
          (candidate) =>
            candidate.kind === 'workflow_review' &&
            candidate.entity === run &&
            !candidate.discarded,
        )) {
          const loaded = await recovery.read(root, 'project', row.id)
          if (!loaded.ok) throw new Error(describe(loaded.error))
          const raw = JSON.parse(loaded.data.record.input.raw)
          if (!raw.pending) continue
          if (!isPendingReview(raw.pending, run))
            throw new Error(
              'An earlier review copy has invalid request fields. Inspect Recovery copies before continuing.',
            )
          const pending = raw.pending as PendingReview
          const found = await gateway.resourceRequest('queryCodingWorkflows', root, {
            operation: 'receipt',
            request_id: pending.request,
          })
          if (!found.ok) throw new Error(describe(found.error))
          const receipt = (found.data as { receipt: unknown }).receipt
          if (receipt) {
            if (!matchesReviewReceipt(found.data, pending))
              throw new Error('An earlier review receipt does not match its input.')
            continue
          }
          // A later durable revision fences this old uncommitted request by CAS.
          if (s.review && pending.expected < s.review.revision) continue
          s.pending = pending
          s.draft = {
            criterion_id: pending.input.criterion_id,
            note: pending.input.note,
            decision: pending.input.decision,
          }
          s.notice =
            'Recovered an unresolved review request. Check its original receipt before another decision.'
          backupReview(root)
          return
        }
        after = page.next
        if (after && (seen.has(after) || seen.size >= 64))
          throw new Error('Recovery listing could not be reconciled completely.')
        if (after) seen.add(after)
      } while (after)
    } catch (error) {
      s.error = String(error)
      if (s.review) s.review.can_review = false
    } finally {
      s.busy = false
    }
  }
  async function refresh(root: string) {
    const s = session(root),
      run = s.run
    if (!run || s.busy) return
    s.busy = true
    try {
      const r = await gateway.resourceRequest('queryCodingWorkflows', root, {
        operation: 'review',
        run_id: run,
      })
      if (s.run !== run) return
      if (r.ok) {
        s.review = r.data as WorkflowReview
        s.error = ''
      } else {
        s.review = null
        s.error = describe(r.error)
      }
    } finally {
      s.busy = false
    }
    if (!s.pending && s.review && s.run === run) await recoverOutstandingReview(root, run)
  }
  function backupReview(root: string) {
    const s = session(root)
    s.reviewDrafts[s.run] = { pending: s.pending, draft: s.draft }
    recovery.edit(root, 'workflow_review', s.run, {
      raw: JSON.stringify({ run: s.run, pending: s.pending, draft: s.draft }),
      pending_submission: s.pending?.request ?? null,
    })
  }
  function editReview(root: string, criterion_id: string, note: string) {
    const s = session(root)
    s.draft = criterion_id ? { criterion_id, note, decision: 'accepted' } : null
    backupReview(root)
  }
  async function decide(root: string, choice: ReviewDecision) {
    const s = session(root)
    if (s.busy || s.pending || !s.review?.can_review) return
    s.pending = {
      request: crypto.randomUUID(),
      expected: s.review.revision,
      input: { ...clone(choice), run_id: s.run, evidence_digest: s.review.digest },
    }
    backupReview(root)
    await submitReview(root)
  }
  async function submitReview(root: string) {
    const s = session(root),
      pending = s.pending
    if (!pending || s.busy) return
    s.busy = true
    s.error = ''
    try {
      if (!(await recovery.flush(root, 'workflow_review', pending.input.run_id))) {
        s.error = 'Review input is not backed up. Keep this draft and retry the same request.'
        return
      }
      const r = await gateway.resourceRequest(
        'recordWorkflowReview',
        root,
        pending.input,
        pending.expected,
        pending.request,
      )
      if (!r.ok) {
        let definitive = false
        try {
          definitive = typeof JSON.parse(r.error).code === 'string'
        } catch {
          /* transport outcome unknown */
        }
        s.error = describe(r.error)
        if (definitive) {
          s.pending = null
          backupReview(root)
        } else
          s.error = 'Review outcome unknown. Check the original receipt before another decision.'
        return
      }
      if (!matchesReviewReceipt(r.data, pending)) {
        s.error = 'No durable review receipt returned. Reconcile the original request.'
        return
      }
      s.pending = null
      s.draft = null
      backupReview(root)
      s.notice = 'Decision recorded. Execution permissions and task limits are unchanged.'
    } finally {
      s.busy = false
    }
    await refresh(root)
  }
  async function reconcile(root: string) {
    const s = session(root),
      pending = s.pending
    if (!pending || s.busy) return
    s.busy = true
    try {
      const r = await gateway.resourceRequest('queryCodingWorkflows', root, {
        operation: 'receipt',
        request_id: pending.request,
      })
      if (!r.ok) {
        s.error = describe(r.error)
        return
      }
      const receipt = (r.data as { receipt: unknown }).receipt
      if (receipt) {
        if (!matchesReviewReceipt(r.data, pending)) {
          s.error =
            'The receipt does not match this decision. Keep the original request for reconciliation.'
          return
        }
        s.pending = null
        s.draft = null
        backupReview(root)
        s.notice = 'Original review receipt recovered.'
      } else {
        s.notice = 'No receipt found. Retrying uses the same request identity.'
      }
    } finally {
      s.busy = false
    }
    if (s.pending) await submitReview(root)
    else await refresh(root)
  }
  function restore(root: string, record: RecoveryRecord) {
    const s = session(root)
    s.error = ''
    if (s.busy) return
    try {
      const raw = JSON.parse(record.input.raw)
      if (
        record.input.kind === 'workflow' &&
        raw.input?.instance_id === record.input.entity &&
        isTemplateDraft(raw.input)
      ) {
        if (
          raw.source &&
          (typeof raw.source.session !== 'string' || typeof raw.source.entity !== 'string')
        )
          throw new Error('Invalid source conversation in the recovery copy.')
        if (raw.saved && typeof raw.saved !== 'string')
          throw new Error('Invalid saved blueprint path.')
        s.plan = {
          ...initial(),
          input: raw.input,
          source: raw.source ?? null,
          saved: raw.saved ?? '',
        }
        s.mode = 'plan'
        backupPlan(root)
        void catalog(root)
      } else if (record.input.kind === 'workflow_review' && raw.run === record.input.entity) {
        if (raw.pending && !isPendingReview(raw.pending, raw.run))
          throw new Error('Review request identity does not match its run.')
        if (raw.draft && !isReviewDecision(raw.draft))
          throw new Error('Invalid review draft fields.')
        s.run = raw.run
        s.pending = raw.pending ?? null
        s.draft = raw.draft ?? null
        s.mode = 'review'
        backupReview(root)
        void refresh(root)
      } else throw new Error('Choose a workflow plan or review copy.')
      s.notice = 'Restored as a new input copy. Saved versions and evidence must be checked again.'
    } catch (e) {
      s.error = String(e)
    }
  }
  return {
    current,
    session,
    openPlan,
    changed,
    preview,
    create,
    openReview,
    refresh,
    decide,
    reconcile,
    restore,
    backupPlan,
    editReview,
  }
})
