# Metteur

A Visualization-First Agent Based on the Plan-and-Execute Paradigm.

## Running the tests

```powershell
powershell -File scripts/test.ps1          # cargo test -j 1 --workspace
powershell -File scripts/test.ps1 -p metteur-daemon
```

`cargo test` needs the MSVC environment on `PATH` (otherwise the linker resolves to
Git for Windows' GNU `link.exe` and linking fails) and the daemon's test binary is
large enough that parallel linking is unsafe, so the script always passes `-j 1`.
The web client has its own suites: `pnpm typecheck`, `pnpm lint`, `pnpm test:e2e`,
`pnpm test:monkey` in `modules/webcore`.

## Oversight and scoped delegation

During a blueprint run, open **Execution → Supervision → Chat** for the concierge.
It requires an explicit `oversight.concierge_model`. Requests retain their original
text and source; the **Requests** and **Reviews** tabs show their actual status.
Finished runs remain readable. Direct pause, resume and stop do not wait for a model.

Oversight defaults to `assisted`. `off` permits reports but no model actions.
`autonomous` still requires explicit user-configured delegation; the default
scope is empty. For example, a user may allow an independent supervisor to pause
or change specific data keys on a specific future node:

```toml
config_version = 2

[oversight]
mode = "autonomous"

[oversight.delegation]
pause_run = true

[[oversight.delegation.blueprint_edits]]
node_id = "00000000-0000-0000-0000-000000000001" # Replace with the actual node ID.
fields = ["prompt", "temperature"] # Exact, case-sensitive node.data keys.
```

Removing a grant, setting `blueprint_edits = []`, or setting `pause_run = false`
revokes that scope. The server checks it again before applying an action. Ask mode
still requires confirmation. Concierge-derived or merged requests, circuit
reruns, and whole-run cancellation always need a fresh concrete user decision.
Structural edits are unsupported. Delegation creates no user approval record
and does not change the sandbox permissions used by executed nodes.

`OversightCheckpoint` uses `data.async = false` by default. Synchronous gates wait
for the real report and any approved application. Failures and unhandled concerns
wait for a human disposition. An explicitly connected Switch can route `Verdict`.
Async gates return `pending` and a stable `ReviewId`; later reports never rewrite
past downstream outputs. An applied action is separate from a validation pass.

After an interruption, workspace recovery closes unfinished reviews and old
confirmations. Requests and token reservations are independent of execution
checkpoints and are never reset by restoring an older checkpoint. Recovery can
complete an acknowledgement only when the recorded Version Flow snapshot and
committed checkpoint prove the application; it never repeats the edit. A proven
unchanged intent is retired and needs a new proposal and decision. Inconsistent
file, version or scheduling evidence blocks execution and requires manual recovery.
The workspace remains open for inspection, with unresolved outcomes visible in
Requests and Reviews.

A recorded stop prevents resuming the same run even if the final cancellation
checkpoint was interrupted. Reviews distinguishes the stop receipt, recorded
checkpoint and actual rollback outcome; an unavailable outcome is not a successful
rollback. Existing file recovery only covers recorded file changes, not arbitrary
commands or network effects, and does not promise exactly-once external execution.
