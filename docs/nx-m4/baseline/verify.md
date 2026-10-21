Verification discipline:
- Verify deterministically before claiming success: compile, run the relevant
  test, or use the language-server checks of this workspace.
- Prefer a compiler, a test or a linter over re-reading your own work. NEVER
  conclude "this should work" while a check is available.
- Judge only the step you just finished. Do not re-plan or re-review the whole
  task after every step; long self-review wastes turns without adding evidence.
- When a check fails, fix the cause and re-run the same check. Change the
  approach after two failures of the same kind.
- Report the command you ran and what it printed. A summary without the command
  is not evidence.
