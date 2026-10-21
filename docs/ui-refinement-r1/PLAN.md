# Usability refinement R1 — approved integration

User request 2026-10-15 14:18:49 UTC: remove redundant small explanatory text from every new page; important functions at the top of the left rail; drag-adjustable persistent order; further experience-focused improvements.

Base: 9d6e4ad41370336d0dacfb3ea089f9b68a41114c. Original source and bundle preserved. Preview approved by user at 2026-10-15 14:37:04 UTC (Sentinel_a0e8332146188191801c9db3184e534f). R1 is now integrated into the production frontend; the isolated preview remains preserved.

## Scope
- Default rail: Chat, Explorer, Execution, Task continuation, Project memory, Prompt resources, Schedules, Version Flow, Addon pages. Settings remains pinned.
- Real shared rail component: pointer drag, before/after indicators, local persistence, reset order; Alt+Up/Down and an accessible menu also support keyboard/touch. Unknown/duplicate/removed saved IDs cannot remove navigation. New items append safely.
- Remove duplicated uppercase eyebrows and always-visible teaching paragraphs from all new surfaces. Purpose/help moves to keyboard-focusable tooltips. Keep field values, status, source records, errors, exact authority/budget/irreversible-action confirmations visible where needed.
- Memory: compact heading/search, readable note body, source/digest/file metadata under Source details. Keep current vs verified distinction available from status help; do not hide a stale-version warning.
- Prompts: aligned Project/Personal and search toolbar; clearer Use in Chat action (select, open composer, no automatic send); reduce empty drop-target scaffolding. Keep actual selections and source revision checks, not just the visuals.
- Tasks/schedules: consistent compact headers and action positions; readable budget/wait state; technical details on demand. Avoid dead-end actions without an eligible task. Keep Stop consequences and plan confirmation explicit.
- Blueprint run review: remove decorative tagline; retain concrete scope, dependency versions, source permission, completion policy and final confirmation.
- Addon inspector: remove duplicate page eyebrows/footer prose; scoped explanation on demand; retain evidence states, target revision and explicit independent human acceptance.

## Evidence so far
Vue typecheck and isolated build passed. Ten order-helper assertions passed. Actual cloud-browser Alt+Up move survives reload; actual native pointer drag moved Addon pages from last to second; reset restored defaults. Preview storage key is separate from the production preference.

This preview's resource/task records are synthetic. Non-navigation action buttons demonstrate intended placement and do not execute RPCs. Permission and budget dialogs retain production semantics. Next: present concrete screenshots, obtain integration confirmation, then connect approved refinements to existing production data and perform only necessary UI checks.


## Integration verification
- Production Vite build passes; clean output uses 243 precache entries. Previous build evidence is preserved.
- Shared rail is wired to existing App routing. Browser observed default order, keyboard reordering retained after reload, and reset. The same rail’s native pointer drag was verified in the approved preview.
- Memory: More actions → Edit note → Cancel, source disclosure, compact search/type controls.
- Prompt: search/no-match/clear, existing resource selection → Chat composer with a versioned chip and no automatic send. Test selection removed afterward.
- Tasks: existing completed local task; budget and state visible. New-task dialog opened/closed without starting a task.
- Schedules: no eligible saved task offers Open tasks instead of a dead-end creation form. Existing creation/edit/confirmation logic retained. Polls no longer overlap.
- Addon: signed local sample regenerated with the public deterministic fixture key and installed without new permissions. One directed lifecycle test passes; local offline report shows six checks and an on-demand scope disclosure. No model review or human acceptance executed.
- Blueprint approval: decorative explanations moved to help; permission/source/budget/confirmation retained. No new model proposal generated for cosmetic verification.

## Important typecheck correction
The root `vue-tsc --noEmit` command uses an empty references-only tsconfig and is not evidence of application type safety. Direct `-p tsconfig.app.json --ignoreDeprecations 6.0` reveals 36 diagnostic lines on the preserved preview baseline and 33 after this integration. Comparison finds no new diagnostic; three existing App/Memory diagnostics were removed. Global typecheck is NOT passing. Existing CodeMirror dependency duplication, core/config typing, concierge and execution typing remain recorded rather than silently claiming completion. This UI-directed pass does not broaden into an unrelated full-repository migration.

## Remaining boundaries
Native Windows and real provider/model validation remain pending as before. Final human visual acceptance remains with the owner. No paid API, origin push or merge.
