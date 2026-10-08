import { test, expect, type Page } from '@playwright/test'

async function openRun(page: Page, chatSurface = false) {
  await page.goto('/')
  await page.getByLabel('Workspace path').fill('D:/metteur-demo/concierge')
  await page.getByRole('button', { name: 'Open', exact: true }).click()
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible()
  await page.evaluate(async (chat) => {
    const c = '/src/core/index.ts', r = '/src/router.ts', w = '/src/lib/workspace-url.ts'
    const { gateway } = await import(c)
    gateway.connected.value = true
    const data = { run_id: 'concierge-run', conversation_id: 'concierge-run', available: true, read_only: false, reason: '', consumer_enabled: false, messages: [], requests: [] }
    const fixture = gateway.conciergeTest = { data, active: true, sent: 0, approvals: 0, ordinary: 0, urgent: [], controls: [], hold: false, holdPoll: false }
    gateway.listExecutions = async () => ({ ok: true, data: [{ runId: data.run_id, status: fixture.active ? 'Running' : 'Completed', startedAt: 1, updatedAt: Date.now(), snapshot: { runtime: { active: fixture.active, pending_approval_ids: [] } } }] })
    gateway.getConciergeState = async () => {
      const result = { ok: true, data: structuredClone(data) }
      if (fixture.holdPoll) return new Promise(resolve => { fixture.releasePoll = () => resolve(result) })
      return result
    }
    gateway.respondApproval = async () => { fixture.approvals++; return { ok: true, data: undefined } }
    gateway.sendChat = async () => { fixture.ordinary++; return { ok: true, data: undefined } }
    gateway.sendInterrupt = async (_ws, text, priority) => { fixture.urgent.push({ text, priority }); return { ok: true, data: undefined } }
    for (const action of ['pause', 'cancel']) gateway[action] = async () => { fixture.controls.push(action); return { ok: true, data: undefined } }
    gateway.sendConciergeMessage = async (_ws, run, conversation, id, text, event) => {
      fixture.sent++
      const turn = { id, conversation_id: conversation, original_text: text, at_ms: 1, state: 'processing', answer: '', request_id: null, error: null }
      data.messages.push(turn)
      event({ runId: run, messageId: id, kind: 'processing', turn: structuredClone(turn) })
      const finish = () => {
        Object.assign(turn, { state: 'received', request_id: id, answer: 'Received; not processed. This request grants no permission.' })
        data.requests.push({ request_id: id, run_id: run, conversation_id: conversation, original_text: text, concierge_note: '<img src=x onerror=alert(1)> User asks to stop', state: 'received', source: 'concierge_forwarded', review_id: null, proposals: [], result_refs: [] })
        event({ runId: run, messageId: id, kind: 'received', turn: structuredClone(turn) })
        return { ok: true, data: undefined }
      }
      if (fixture.hold) return new Promise(resolve => { fixture.finish = () => resolve(finish()) })
      return finish()
    }
    await (await import(r)).router.push((await import(w)).wurl(chat ? '/chat' : '/execution'))
  }, chatSurface)
  // The Chat surface also contains Run concierge. Confirm the actual surface
  // through the user navigation before asserting supervision controls.
  if (!chatSurface) {
    await page.getByRole('button', { name: 'Execution', exact: true }).click()
    await expect(page).toHaveURL(/\/execution$/)
    await expect(page.getByRole('button', { name: 'Reviews', exact: true })).toBeVisible()
  }
  await expect(page.getByRole('region', { name: 'Run concierge' })).toBeVisible()
}

async function proposalFixture(page: Page) {
  await openRun(page)
  await page.evaluate(async () => {
    const c = '/src/core/index.ts', g = (await import(c)).gateway, t = g.conciergeTest
    const base = { snapshot_id: 'base-snapshot', blueprint_uri: 'plans/proposal.blueprint', blob_hash: 'base-hash-0123456789' }
    const result = { ...base, snapshot_id: 'result-snapshot', blob_hash: 'result-hash-0123456789' }
    const proposal = t.proposal = { proposal_id: 'full-proposal', review_id: 'full-review', run_id: 'concierge-run', source_request_ids: ['full-request'], original_requests: [{ request_id: 'full-request', source: 'concierge_forwarded', original_text: 'Change this future prompt; ask me first.' }], kind: 'blueprint_edits', state: 'awaiting_confirmation', reason: '', decision_source: '', result_refs: [], result_version: null,
      binding: { source: 'supervisor', scope: 'root-blueprint', summary: 'Revise the future prompt', base, affected_nodes: ['future-node'], before: [{ id: 'future-node', kind: 'CallLLM', data: { prompt: 'Old prompt' } }], after: [{ id: 'future-node', kind: 'CallLLM', data: { prompt: '<img src=x onerror=alert(1)> New prompt\n' + 'Long proposed field.\n'.repeat(80) } }] } }
    t.resultVersion = result
    t.data.consumer_enabled = true
    t.data.requests = [{ request_id: 'full-request', run_id: 'concierge-run', conversation_id: 'concierge-run', original_text: 'Change this future prompt; ask me first.', concierge_note: 'Change requested', state: 'awaiting_confirmation', source: 'concierge_forwarded', review_id: 'full-review', proposals: [{ proposal_id: 'full-proposal', state: 'awaiting_confirmation', result_refs: [] }], result_refs: [] }]
    t.data.reports = [{ review_id: 'full-review', run_id: 'concierge-run', status: 'running', verdict: null, summary: '', source_request_ids: ['full-request'], triggers: ['request'], actual_action_refs: [], work: { model: 'supervisor', notes: [], answers: [], evidence: [] }, proposals: [proposal], usage: [{ id: 'old-call', model: 'legacy model', charged: 115, state: 'reported', cost_micros: 98765432, currency: 'USD' }, { id: 'new-call', model: 'current model', charged: 110, state: 'reported', accounting_version: 1, cost_micros: 1000, currency: 'USD' }] }]
    g.listOversightReports = async () => {
      const response = { ok: true, data: { run_id: 'concierge-run', reports: structuredClone(t.data.reports) } }
      if (t.holdReports) return new Promise(resolve => { t.releaseReports = () => resolve(response) })
      return response
    }
    g.listSnapshots = async () => ({ ok: true, data: [base, result].map((v, index) => ({ id: v.snapshot_id, createdAt: index + 1, message: v.snapshot_id, fileCount: 1 })) })
    g.listFileHistory = async (ws, path) => { t.historyRead = { ws, path }; return { ok: true, data: [] } }
    g.listExecutions = async () => ({ ok: true, data: [{ runId: 'concierge-run', status: t.active ? 'Running' : 'Completed', startedAt: 1, updatedAt: Date.now(), snapshot: { blueprint_version: result, view: { invocations: [], graphs: {}, edges: [], sequence: 0 }, runtime: { active: t.active, pending_approval_ids: t.pendingApproval ? ['approval-full'] : [] } } }] })
  })
}

test('shared proposal cards preserve confirmation boundaries and navigate future nodes and exact versions', async ({ page }) => {
  await proposalFixture(page)
  await page.getByRole('button', { name: 'Requests', exact: true }).click()
  const card = page.getByRole('article', { name: 'Proposal full-proposal', exact: true })
  await expect(card.getByText('Awaiting your confirmation', { exact: true })).toBeVisible()
  await expect(card).toContainText('Change this future prompt; ask me first.')
  await expect(card.getByLabel('Proposed field changes')).toContainText('data.prompt')
  await expect(card.getByLabel('Proposed field changes')).toContainText('Old prompt')
  await expect(card.locator('img')).toHaveCount(0)
  await card.getByRole('button', { name: 'Inspect node · future-node', exact: true }).click()
  const target = page.getByLabel('Proposal target details')
  await expect(target).toContainText('No recorded invocation for this target.')
  await expect(target).toContainText('Old prompt')
  await expect(target).toContainText('New prompt')
  expect(await page.evaluate(async () => { const c='/src/core/index.ts'; return (await import(c)).gateway.conciergeTest.approvals })).toBe(0)
  await page.evaluate(async () => {
    const c='/src/core/index.ts', e='/src/stores/execution.ts', b='/src/core/blueprint-approval.ts', g=(await import(c)).gateway, t=g.conciergeTest
    t.pendingApproval = true
    g.respondApproval = async (_ws, id, allow) => { t.approvals++; t.pendingApproval = false; t.proposal.state = allow ? 'approved_pending_apply' : 'rejected'; t.proposal.decision_source = 'human'; t.data.requests[0].state = t.proposal.state; return { ok: true, data: undefined } }
    const plan=(await import(b)).blueprintApproval({ ...t.proposal.binding, request_type:'replan_proposal', run_id:'concierge-run', proposal_id:'full-proposal', original_requests:t.proposal.original_requests })
    ;(await import(e)).useExecutionStore().approval = { id:'approval-full', ...plan, tool:'ProposeBlueprintEdits', requestType:'replan_proposal' }
  })
  await page.getByRole('dialog').getByRole('button', { name: 'Allow', exact: true }).click()
  await expect(card.getByText('Confirmed · not applied', { exact: true })).toBeVisible()
  await expect(card.getByRole('button', { name: /Result version/ })).toHaveCount(0)
  await page.evaluate(async () => { const c='/src/core/index.ts', t=(await import(c)).gateway.conciergeTest; t.proposal.state='applied'; t.proposal.result_version=t.resultVersion; t.proposal.result_refs=['version:result-snapshot']; t.data.requests[0].state='applied'; t.data.reports[0].status='completed'; t.data.reports[0].verdict='action_taken'; t.active=false; t.data.read_only=true })
  await expect(card.getByText('Applied', { exact: true })).toBeVisible()
  await page.getByRole('button', { name:'Reviews', exact:true }).click()
  await expect(card.getByText('Applied', { exact: true })).toBeVisible()
  await expect(page.getByRole('region', { name:'Supervisor reviews' })).toContainText('Historical budget accounting · cost unavailable')
  await expect(page.getByRole('region', { name:'Supervisor reviews' })).not.toContainText('98.765432')
  await expect(page.getByRole('region', { name:'Supervisor reviews' })).toContainText('0.001 USD')
  for (const kind of ['Base', 'Result']) {
    await card.getByRole('button', { name: new RegExp(`^${kind} version ·`) }).click()
    const reference=page.getByLabel('Referenced blueprint version')
    await expect(reference).toContainText(`${kind.toLowerCase()}-hash-0123456789`)
    await expect(reference).toContainText('full-proposal')
    expect(await page.evaluate(async () => { const c='/src/core/index.ts'; return (await import(c)).gateway.conciergeTest.historyRead.path })).toBe('plans/proposal.blueprint')
    expect(await page.evaluate(async () => { const v='/src/stores/version.ts'; return (await import(v)).useVersionStore().selectedId })).toBe(`${kind.toLowerCase()}-snapshot`)
    await page.evaluate(async () => { const r='/src/router.ts', w='/src/lib/workspace-url.ts'; await (await import(r)).router.push((await import(w)).wurl('/execution')) })
    await page.getByRole('button', { name:'Reviews', exact:true }).click()
    await expect(card.getByText('Applied', { exact:true })).toBeVisible()
  }
  expect(await page.evaluate(async () => { const c='/src/core/index.ts'; return (await import(c)).gateway.conciergeTest.approvals })).toBe(1)
})

test('proposal history distinguishes failure and delegation, scrolls within themes, and rejects late workspace data', async ({ page }) => {
  await proposalFixture(page)
  await page.getByRole('button', { name:'Reviews', exact:true }).click()
  const card=page.getByRole('article', { name:'Proposal full-proposal', exact:true })
  await expect(card).toBeVisible()
  for (const state of ['rejected', 'failed', 'closed_unhandled', 'approved_pending_apply', 'applied']) {
    await page.evaluate(async value => { const c='/src/core/index.ts', t=(await import(c)).gateway.conciergeTest; t.proposal.state=value; t.proposal.decision_source=value==='applied'?'delegated':'human'; t.proposal.diagnostic=value==='failed'?{ category:'approval_expired',stage:'approval',message:'The confirmation expired.',run_id:'concierge-run',review_id:'full-review' }:null },state)
    await expect(card).toContainText(({ rejected:'Rejected', failed:'Failed', closed_unhandled:'Closed · unhandled', approved_pending_apply:'Confirmed · not applied', applied:'Applied' })[state]!)
    if(state==='failed') await expect(card).toContainText('approval expired')
  }
  await expect(card).toContainText('Decision: existing user delegation')
  await expect(card).toContainText('Result version unavailable')
  await page.emulateMedia({ reducedMotion:'reduce' })
  for(const dark of [false,true]) {
    await page.evaluate(value=>document.documentElement.classList.toggle('dark',value),dark)
    await page.setViewportSize({ width:800,height:760 })
    const diff=card.getByLabel('Proposed field changes')
    await expect(diff).toBeVisible()
    expect(await diff.evaluate(el=>el.scrollHeight>el.clientHeight && el.clientHeight<=300)).toBe(true)
    await page.screenshot({path:`../../.tmp/r06-proposal-${dark?'dark':'light'}.png`})
  }
  await page.evaluate(async()=>{const c='/src/core/index.ts'; (await import(c)).gateway.conciergeTest.holdReports=true})
  await expect.poll(()=>page.evaluate(async()=>{const c='/src/core/index.ts';return !!(await import(c)).gateway.conciergeTest.releaseReports})).toBe(true)
  await page.evaluate(async()=>{const c='/src/core/index.ts';(await import(c)).gateway.connected.value=false})
  await expect(card).toHaveCount(0)
  await page.evaluate(async()=>{const c='/src/core/index.ts';(await import(c)).gateway.conciergeTest.releaseReports()})
  await expect(card).toHaveCount(0)
  await page.evaluate(async()=>{
    const c='/src/core/index.ts',w='/src/stores/workspace.ts',v='/src/stores/version.ts',g=(await import(c)).gateway,ws=(await import(w)).useWorkspaceStore(),version=(await import(v)).useVersionStore()
    g.listFileHistory=async()=>new Promise(resolve=>{g.conciergeTest.releaseHistory=()=>resolve({ok:true,data:[{path:'foreign.secret'}]})})
    void version.pickFile('old.blueprint')
    ws.active={...ws.active,path:'D:/metteur-demo/other-workspace'}
    await Promise.resolve(); g.conciergeTest.releaseHistory()
  })
  await expect.poll(()=>page.evaluate(async()=>{const v='/src/stores/version.ts',s=(await import(v)).useVersionStore();return {history:s.history.length,reference:s.reference,selected:s.selectedId}})).toEqual({history:0,reference:null,selected:null})
})

test('partial terminal requests keep applied facts separate from unavailable legacy details and mismatched runs', async ({ page }) => {
  await proposalFixture(page)
  await page.evaluate(async () => {
    const c='/src/core/index.ts', t=(await import(c)).gateway.conciergeTest
    t.active=false; t.data.read_only=true; t.data.reason='Run is read-only'
    t.proposal.state='applied'; t.proposal.result_version=t.resultVersion
    t.data.requests[0].state='applied'
    t.data.requests.push({ ...t.data.requests[0], request_id:'unhandled-request', review_id:'missing-old-review', state:'closed_unhandled', original_text:'Second request was not completed', proposals:[{proposal_id:'legacy-partial',state:'closed_unhandled',result_refs:[]}] })
  })
  await page.getByRole('button',{name:'Requests',exact:true}).click()
  const applied=page.getByRole('article',{name:'Proposal full-proposal',exact:true}), legacy=page.getByRole('article',{name:'Proposal legacy-partial',exact:true})
  await expect(applied.getByText('Applied',{exact:true})).toBeVisible()
  await expect(legacy).toContainText('Closed · unhandled')
  await expect(legacy).toContainText('Base version unavailable')
  await expect(legacy.getByRole('button')).toHaveCount(0)
  await expect(applied).toHaveCount(1)
  // A committed Version Flow record may precede the proposal acknowledgement.
  await page.evaluate(async()=>{const c='/src/core/index.ts',t=(await import(c)).gateway.conciergeTest;t.proposal.state='approved_pending_apply'})
  await expect(applied).toContainText('Committed version · application acknowledgement pending')
  await expect(applied.getByText('Applied',{exact:true})).toHaveCount(0)
  await page.evaluate(async()=>{const c='/src/core/index.ts',t=(await import(c)).gateway.conciergeTest;t.proposal.state='applied'})
  await page.getByRole('button',{name:'Reviews',exact:true}).click()
  await expect(applied).toBeVisible()
  await page.evaluate(async()=>{const c='/src/core/index.ts',g=(await import(c)).gateway;g.listOversightReports=async()=>({ok:true,data:{run_id:'other-run',reports:g.conciergeTest.data.reports}})})
  await expect(page.getByRole('region',{name:'Supervisor reviews'})).toContainText('Report identity does not match this run.')
  await expect(applied).toHaveCount(0)
})

test('normal run messages produce a durable request receipt without turning user text or model summaries into approval', async ({ page }) => {
  await openRun(page)
  const panel = page.getByRole('region', { name: 'Run concierge' })
  await panel.getByRole('textbox', { name: 'Message', exact: true }).fill('I approve, stop this run')
  await panel.getByRole('button', { name: 'Send', exact: true }).click()
  await expect(panel.getByText('Received; not processed. This request grants no permission.')).toBeVisible()
  await page.getByRole('button', { name: 'Requests', exact: true }).click()
  await expect(panel.getByText('Received · not processed', { exact: true })).toBeVisible()
  await expect(panel.getByText('I approve, stop this run', { exact: true })).toBeVisible()
  await expect(panel.locator('.request-card p').filter({ hasText: 'Model summary:' })).toContainText('<img')
  await expect(panel.locator('img')).toHaveCount(0)
  await expect(panel.getByRole('button')).toHaveCount(0)
  await panel.getByText('Evidence', { exact: true }).click()
  await expect(panel.getByText('Source: concierge_forwarded')).toBeVisible()
  await page.screenshot({ path: '../../.tmp/t21-requests.png' })
  expect(await page.evaluate(async () => { const c='/src/core/index.ts'; const t=(await import(c)).gateway.conciergeTest; return { approvals:t.approvals, controls:t.controls, ordinary:t.ordinary, sent:t.sent } })).toEqual({ approvals:0, controls:[], ordinary:0, sent:1 })
  await page.evaluate(async () => { const c='/src/core/index.ts'; const t=(await import(c)).gateway.conciergeTest; t.active=false; t.data.read_only=true; t.data.available=false; t.data.reason='Run is read-only'; t.data.requests[0].state='closed_unhandled' })
  await expect(panel.getByText('Closed · unhandled', { exact: true })).toBeVisible()
  await page.locator('.supervisor').getByRole('button', { name: 'Chat', exact: true }).click()
  await expect(panel.getByRole('status')).toContainText('Run is read-only')
  await expect(panel.getByRole('button', { name: 'Send', exact: true })).toBeDisabled()

})

test('budget exhaustion and an in-flight concierge do not block direct urgent pause or stop controls', async ({ page }) => {
  await openRun(page)
  const panel = page.getByRole('region', { name: 'Run concierge' }), input = panel.getByRole('textbox', { name: 'Message', exact: true })
  await page.evaluate(async () => { const c='/src/core/index.ts'; (await import(c)).gateway.conciergeTest.hold=true })
  await input.fill('Explain the current node')
  await input.press('Enter')
  await expect(panel.getByText('Processing — no request has been confirmed.')).toBeVisible()
  await input.fill('Urgent while pending')
  await input.press('Control+Enter')
  await page.getByRole('button', { name: 'Pause', exact: true }).click()
  await page.evaluate(async () => { const c='/src/core/index.ts'; const t=(await import(c)).gateway.conciergeTest; t.finish(); t.data.available=false; t.data.reason='Oversight token budget exhausted' })
  await expect(panel.getByRole('status')).toContainText('Oversight token budget exhausted')
  await input.fill('Urgent after budget exhausted')
  await expect(panel.getByRole('button', { name: 'Send', exact: true })).toBeDisabled()
  await input.press('Control+Enter')
  await page.getByRole('button', { name: 'Stop', exact: true }).click()
  expect(await page.evaluate(async () => { const c='/src/core/index.ts'; const t=(await import(c)).gateway.conciergeTest; return { sent:t.sent, urgent:t.urgent, controls:t.controls } })).toEqual({ sent:1, urgent:[{text:'Urgent while pending',priority:'Urgent'},{text:'Urgent after budget exhausted',priority:'Urgent'}], controls:['pause','cancel'] })
})

test('the chat surface routes an active blueprint to concierge and ignores late replies after disconnect', async ({ page }) => {
  await openRun(page, true)
  const panel = page.getByRole('region', { name: 'Run concierge' })
  await expect(panel.locator('.chat-composer-options')).toHaveCount(0)
  await page.evaluate(async () => { const c='/src/core/index.ts'; const t=(await import(c)).gateway.conciergeTest; t.hold=true; t.holdPoll=true })
  await panel.getByRole('textbox', { name: 'Message', exact: true }).fill('Late request')
  await panel.getByRole('button', { name: 'Send', exact: true }).click()
  await expect.poll(() => page.evaluate(async () => { const c='/src/core/index.ts'; return !!(await import(c)).gateway.conciergeTest.releasePoll })).toBe(true)
  await page.evaluate(async () => { const c='/src/core/index.ts'; (await import(c)).gateway.connected.value=false })
  await expect(panel.getByRole('status')).toContainText('Disconnected')
  await page.evaluate(async () => { const c='/src/core/index.ts'; const t=(await import(c)).gateway.conciergeTest; t.finish(); t.releasePoll() })
  await expect(panel.getByText('Received; not processed. This request grants no permission.')).toHaveCount(0)
  await page.evaluate(async () => { const c='/src/core/index.ts'; const g=(await import(c)).gateway; g.conciergeTest.active=false; g.connected.value=true })
  await expect(panel).toHaveCount(0)
  await expect(page.locator('.chat-composer-options')).toBeVisible()
  expect(await page.evaluate(async () => { const c='/src/core/index.ts'; return (await import(c)).gateway.conciergeTest.ordinary })).toBe(0)
})

test('a slow history read cannot erase a newer streamed receipt', async ({ page }) => {
  await openRun(page)
  const panel = page.getByRole('region', { name: 'Run concierge' })
  await page.evaluate(async () => { const c='/src/core/index.ts'; (await import(c)).gateway.conciergeTest.holdPoll=true })
  await expect.poll(() => page.evaluate(async () => { const c='/src/core/index.ts'; return !!(await import(c)).gateway.conciergeTest.releasePoll })).toBe(true)
  await panel.getByRole('textbox', { name: 'Message', exact: true }).fill('Keep this recorded request')
  await panel.getByRole('button', { name: 'Send', exact: true }).click()
  await expect(panel.getByText('Received; not processed. This request grants no permission.')).toBeVisible()
  await page.evaluate(async () => { const c='/src/core/index.ts'; (await import(c)).gateway.conciergeTest.releasePoll(); await new Promise(requestAnimationFrame) })
  await expect(panel.getByText('Keep this recorded request', { exact: true })).toBeVisible()
  await expect(panel.getByText('Received; not processed. This request grants no permission.')).toBeVisible()
})

test('reviews show truthful failures, escaped opinions and persisted evidence', async ({ page }) => {
  await openRun(page)
  await page.evaluate(async () => {
    const c = '/src/core/index.ts', g = (await import(c)).gateway
    g.listOversightReports = async () => ({ ok: true, data: { run_id: 'concierge-run', reports: [
      { review_id: 'failed-review', status: 'timed_out', verdict: null, summary: 'Remote usage may continue.', triggers: ['request'], source_request_ids: ['request-1'], actual_action_refs: [], work: { model: 'supervisor', notes: [], answers: [], evidence: [] }, usage: [{ id: 'call', model: 'supervisor', charged: 2048, state: 'in_flight_unknown', cost_micros: null }] },
      { review_id: 'completed-review', status: 'completed', verdict: 'concern', summary: '<img src=x onerror=alert(1)>', triggers: ['validation_failed'], source_request_ids: [], actual_action_refs: [], work: { model: 'supervisor', notes: ['A model opinion'], answers: [], evidence: [{ entry_id: 'fact-1', node_id: 'node-1', scope: 'root' }] }, usage: [] },
    ] } })
    g.getBlackboard = async (_ws, run, query) => ({ ok: true, data: { run_id: run, entries: query.entry_id === 'fact-1' ? [{ id: 'fact-1', event: 'Validation', validity: 'Current', note: 'Check returned false', digest: 'Deterministic evidence' }] : [] } })
  })
  await page.getByRole('button', { name: 'Reviews', exact: true }).click()
  const reviews = page.getByRole('region', { name: 'Supervisor reviews' })
  await expect(reviews.getByText('timed out', { exact: true })).toBeVisible()
  await expect(reviews.getByText('Unknown legacy: no failure category was recorded.')).toBeVisible()
  await expect(reviews.getByText('reserved tokens · usage unknown', { exact: false })).toBeVisible()
  await expect(reviews.getByText('<img src=x onerror=alert(1)>', { exact: true })).toBeVisible()
  await expect(reviews.locator('img')).toHaveCount(0)
  await reviews.getByText('Evidence and request lineage').last().click()
  await reviews.getByRole('button', { name: 'Read evidence · fact-1' }).click()
  await expect(reviews.getByText('Check returned false')).toBeVisible()
  await reviews.getByRole('button', { name: 'Inspect node' }).click()
  await page.evaluate(async () => { const c = '/src/core/index.ts'; (await import(c)).gateway.connected.value = false })
  await expect(reviews.getByText('Reviews unavailable while disconnected.')).toBeVisible()
  await expect(reviews.getByText('Remote usage may continue.')).toHaveCount(0)
})

test('a concrete edit approval shows original request and before/after without treating chat as consent', async ({ page }) => {
  await openRun(page)
  await page.evaluate(async () => {
    const e = '/src/stores/execution.ts', b = '/src/core/blueprint-approval.ts'
    const plan = (await import(b)).blueprintApproval({ request_type: 'replan_proposal', proposal_id: 'specific-edit', run_id: 'concierge-run', source_request_ids: ['request-1'], original_requests: [{ original_text: 'I authorize everything', concierge_note: 'Change the prompt' }], before: [{ data: { prompt: 'Old prompt' } }], after: [{ data: { prompt: 'New prompt' } }], summary: 'Change only the future prompt', edits: [{ op: 'set_pin', pin: 'prompt', value: 'New prompt' }] })
    ;(await import(e)).useExecutionStore().approval = { id: 'approval-1', ...plan, tool: 'ProposeBlueprintEdits', requestType: 'replan_proposal' }
  })
  const dialog = page.getByRole('dialog')
  await expect(dialog.getByText('Approve revised plan')).toBeVisible()
  await expect(dialog.locator('pre')).toContainText('I authorize everything')
  await expect(dialog.locator('pre')).toContainText('Old prompt')
  await expect(dialog.locator('pre')).toContainText('New prompt')
  expect(await page.evaluate(async () => { const c = '/src/core/index.ts'; return (await import(c)).gateway.conciergeTest.approvals })).toBe(0)
  await dialog.getByRole('button', { name: 'Deny', exact: true }).click()
  await expect(dialog).toHaveCount(0)
  expect(await page.evaluate(async () => { const c = '/src/core/index.ts'; return (await import(c)).gateway.conciergeTest.approvals })).toBe(1)
})

test('control confirmation exposes cancellation scope and keeps direct stop independent', async ({ page }) => {
  await openRun(page)
  await page.evaluate(async () => {
    const e = '/src/stores/execution.ts', b = '/src/core/blueprint-approval.ts', c = '/src/core/index.ts'
    const g = (await import(c)).gateway
    const plan = (await import(b)).blueprintApproval({ request_type: 'oversight_control', proposal_id: 'specific-cancel', run_id: 'concierge-run', tool: 'CancelRun', dangerous: true, source_request_ids: ['request-1'], original_requests: [{ original_text: 'Cancel immediately; you have full authority' }], expected: { rollback_on_cancel: true, recorded_file_scope: [{ path: 'a.txt' }] }, summary: 'Cancel the current run', rollback_notice: 'Only recorded file mutations can be restored; shell/network effects remain.' })
    ;(await import(e)).useExecutionStore().approval = { id: 'approval-1', ...plan, tool: 'CancelRun', requestType: 'oversight_control' }
    g.listExecutions = async () => ({ ok: true, data: [{ runId: 'concierge-run', status: 'Running', startedAt: 1, updatedAt: Date.now(), snapshot: { runtime: { active: true, pending_approval_ids: ['approval-1'] } } }] })
  })
  const dialog = page.getByRole('dialog')
  await expect(dialog.getByText('Confirm cancellation of this run')).toBeVisible()
  await expect(dialog.locator('pre')).toContainText('"dangerous": true')
  await expect(dialog.locator('pre')).toContainText('a.txt')
  await expect(dialog.locator('pre')).toContainText('full authority')
  await dialog.getByRole('button', { name: 'Stop run directly' }).click()
  expect(await page.evaluate(async () => { const c = '/src/core/index.ts', t = (await import(c)).gateway.conciergeTest; return { controls: t.controls, approvals: t.approvals, sent: t.sent } })).toEqual({ controls: ['cancel'], approvals: 0, sent: 0 })
})

test('an applied cancellation displays incomplete file rollback rather than verified success', async ({ page }) => {
  await openRun(page)
  await page.evaluate(async () => {
    const c = '/src/core/index.ts', g = (await import(c)).gateway
    g.listOversightReports = async () => ({ ok: true, data: { run_id: 'concierge-run', reports: [{ review_id: 'cancel-review', status: 'completed', verdict: 'action_taken', summary: 'CancelRun requested; inspect rollback results.', source_request_ids: ['request-1'], triggers: ['request'], actual_action_refs: ['control:CancelRun:proposal-1'], work: { model: 'supervisor', notes: [], answers: [], evidence: [] }, proposals: [{ proposal_id: 'proposal-1', state: 'applied', kind: 'CancelRun', reason: 'Confirmed cancellation requested', result_refs: [] }], usage: [], cancel_result: { rollback_requested: true, restored_operations: null, error: 'Rollback incomplete: a.txt has external changes', files: [{ path: 'b.txt', phase: 'Reverted' }, { path: 'a.txt', phase: 'Conflict' }] } }] } })
  })
  await page.getByRole('button', { name: 'Reviews', exact: true }).click()
  const panel = page.getByRole('region', { name: 'Supervisor reviews' })
  await expect(panel.getByText('File rollback incomplete', { exact: true })).toBeVisible()
  await expect(panel.getByText('a.txt · Conflict')).toBeVisible()
  await expect(panel.getByText('b.txt · Reverted')).toBeVisible()
  await expect(panel.getByText('Recorded file rollback completed')).toHaveCount(0)
})


test('gate dispositions remain separate from model conclusions and action status', async ({ page }) => {
  await openRun(page)
  await page.evaluate(async () => {
    const c = '/src/core/index.ts', g = (await import(c)).gateway
    g.listOversightReports = async () => ({ ok: true, data: { run_id: 'concierge-run', reports: [{ review_id: 'gate-review', status: 'completed', verdict: 'action_taken', model_verdict: 'concern', human_dispositions: [{ node_id: 'gate', at_ms: 1, action: 'continue_without_validation' }], summary: 'Still needs validation', source_request_ids: [], triggers: ['checkpoint'], actual_action_refs: [], work: { model: 'supervisor', notes: [], answers: [], evidence: [] }, usage: [] }] } })
  })
  await page.getByRole('button', { name: 'Reviews', exact: true }).click()
  const panel = page.getByRole('region', { name: 'Supervisor reviews' })
  await expect(panel.getByText('Model conclusion: concern')).toBeVisible()
  await expect(panel.getByText('Human disposition: continue without validation')).toBeVisible()
  await expect(panel.getByText('action taken', { exact: true })).toBeVisible()
})


test('delegated decisions are shown separately from human confirmation', async ({ page }) => {
  await openRun(page)
  await page.evaluate(async () => {
    const c = '/src/core/index.ts', g = (await import(c)).gateway
    g.listOversightReports = async () => ({ ok: true, data: { run_id: 'concierge-run', reports: [{ review_id: 'delegated-review', status: 'completed', verdict: 'action_taken', summary: 'Pause requested', source_request_ids: [], triggers: ['interval'], actual_action_refs: [], work: { model: 'supervisor', notes: [], answers: [], evidence: [] }, proposals: [{ proposal_id: 'proposal', state: 'applied', kind: 'PauseRun', reason: 'Inspect execution outcome', decision_source: 'delegated', result_refs: [] }], usage: [] }] } })
  })
  await page.getByRole('button', { name: 'Reviews', exact: true }).click()
  const panel = page.getByRole('region', { name: 'Supervisor reviews' })
  await expect(panel.getByText('Decision: existing user delegation', { exact: false })).toBeVisible()
  await expect(panel.getByText('human confirmation', { exact: false })).toHaveCount(0)
})

test('diagnostics expose bounded escaped categories, stages and linked identities', async ({ page }) => {
  await openRun(page)
  await page.evaluate(async () => {
    const c = '/src/core/index.ts', g = (await import(c)).gateway
    g.listOversightReports = async () => ({ ok: true, data: { run_id: 'concierge-run', reports: [{
      review_id: 'diagnostic-review', run_id: 'concierge-run', status: 'cancelled', verdict: null,
      summary: 'Proposal rejected', triggers: ['request'], source_request_ids: [], actual_action_refs: [],
      work: { model: 'supervisor', notes: [], answers: [], evidence: [] }, usage: [],
      diagnostic: { category: 'approval_rejected', stage: 'approval', message: '<img src=x onerror=alert(1)>' + 'x'.repeat(500) + 'NOT_SHOWN_TAIL', run_id: 'concierge-run', review_id: 'diagnostic-review', proposal_id: 'proposal-linked', call_id: 'call-linked' },
    }] } })
  })
  await page.getByRole('button', { name: 'Reviews', exact: true }).click()
  const diagnostic = page.getByLabel('Failure diagnostic')
  await expect(diagnostic.getByText('approval rejected', { exact: true })).toBeVisible()
  await expect(diagnostic).toContainText('<img src=x onerror=alert(1)>')
  await expect(diagnostic).not.toContainText('NOT_SHOWN_TAIL')
  await expect(diagnostic.locator('img')).toHaveCount(0)
  await diagnostic.getByText('Diagnostic references').click()
  await expect(diagnostic).toContainText('Proposal: proposal-linked')
  await expect(diagnostic).toContainText('Call: call-linked')
  await page.evaluate(async () => { const c = '/src/core/index.ts'; (await import(c)).gateway.connected.value = false })
  await expect(diagnostic).toHaveCount(0)
})

test('interrupted cancellation exposes the recorded checkpoint without claiming completion', async ({ page }) => {
  await openRun(page)
  await page.evaluate(async () => {
    const c = '/src/core/index.ts', g = (await import(c)).gateway
    g.listOversightReports = async () => ({ ok: true, data: { run_id: 'concierge-run', reports: [], closing: { source: 'user_direct', checkpoint_status: 'Running', recovery_required: true } } })
  })
  await page.getByRole('button', { name: 'Reviews', exact: true }).click()
  const panel = page.getByRole('region', { name: 'Supervisor reviews' })
  await expect(panel.getByText('Cancellation requested', { exact: true })).toBeVisible()
  await expect(panel.getByText('Recorded checkpoint: Running')).toBeVisible()
  await expect(panel.getByText('Final cancellation outcome is unavailable; this run cannot resume.')).toBeVisible()
  await expect(panel.getByText('Recorded file rollback completed')).toHaveCount(0)
})
