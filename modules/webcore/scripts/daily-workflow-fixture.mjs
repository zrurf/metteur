// Scripted local provider only. File mutations and checks still run in the daemon.
const A = '5726e3bf-35c8-5e17-8942-78756af9c2cc'
const B1 = 'b0548572-8998-4d1b-9f71-27d5946b0001',
  B2 = 'b0548572-8998-4d1b-9f71-27d5946b0002'
export function dailyWorkflowReply(messages, nonce) {
  const goal = messages.findLast(
    (m) => m.role === 'user' && /DW_STORY_A|DW_STORY_B_STAGED/.test(String(m.content)),
  )
  if (!goal) return null
  const sourceEvents = messages.filter(
    (m) => m.role === 'user' && String(m.content).includes('Host-delivered source event'),
  ).length
  const story = String(goal.content).includes('DW_STORY_A') ? 'A' : 'B'
  if (sourceEvents >= (story === 'A' ? 1 : 2))
    return {
      final: true,
      content: `DW_STORY_${story}_COMPLETE ${nonce}: review the host-recorded checks and files.`,
      stage: 'finished',
    }
  const phase = story === 'A' ? 1 : sourceEvents >= 1 ? 2 : 1,
    prefix = `daily-${story}-${phase}`
  const seen = (id) => messages.some((m) => m.role === 'tool' && m.tool_call_id === id)
  const path = story === 'A' ? 'shared.py' : 'feature.py'
  const old =
    story === 'A'
      ? '    return value.strip()'
      : phase === 1
        ? '    return []'
        : '    return selected'
  const replacement =
    story === 'A'
      ? '    return value.strip().lower()'
      : phase === 1
        ? '    selected = [item for item in values if query.casefold() in item.casefold()]\n    return selected'
        : '    if limit is None:\n        return selected\n    if limit < 0:\n        raise ValueError("limit must be non-negative")\n    return selected[:limit]'
  let name, args, id
  if (story === 'A' && !seen(`${prefix}-memory`)) {
    name = 'SearchProjectMemory'
    args = { keyword: 'normalization', limit: 10 }
    id = `${prefix}-memory`
  } else if (!seen(`${prefix}-read`)) {
    name = 'ReadFile'
    args = { path }
    id = `${prefix}-read`
  } else if (story === 'A' && !seen(`${prefix}-note`)) {
    name = 'WriteProjectNote'
    args = {
      request_id: 'c52dfc43-22cd-424a-8888-000000000001',
      expected_revision: 0,
      note: {
        id: 'c52dfc43-22cd-424a-8888-000000000002',
        title: 'Normalization investigation',
        body: 'Model opinion: both callers use shared.normalize. Proposed lowercase normalization remains unverified until the saved test completes.',
        files: ['shared.py', 'alpha.py', 'beta.py'],
      },
    }
    id = `${prefix}-note`
  } else if (!seen(`${prefix}-edit`)) {
    name = 'EditFile'
    args = { path, edits: [{ old_string: old, new_string: replacement }] }
    id = `${prefix}-edit`
  } else {
    name = 'RequestBlueprintRun'
    args = {
      blueprint_id: story === 'A' ? A : phase === 1 ? B1 : B2,
      goal: `Validate story ${story}, phase ${phase}`,
      constraints: [
        'Run only the saved synthetic test command',
        'Preserve source files during validation',
      ],
      summary:
        'The saved graph runs unittest and records the actual exit status through Validator.',
      resume_on_completion: true,
      allow_source_read: true,
    }
    id = `${prefix}-launch`
  }
  return {
    final: false,
    stage: `${story}-${phase}`,
    tool: { id, type: 'function', function: { name, arguments: JSON.stringify(args) } },
  }
}
