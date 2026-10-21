# Metteur

A Visualization-First Agent Based on the Plan-and-Execute Paradigm.

## Running the tests

Use Rust **1.99.0** (see `rust-toolchain.toml`), Node **24.13.1**, Python **3.11 or 3.12** (CI: 3.12),
and the package's **pnpm 11.24.0** through Corepack. Cargo and pnpm lockfiles are
required. Linux needs `protobuf-compiler libclang-dev clang cmake pkg-config`.
On Windows, use an x64 MSVC developer terminal with the Windows SDK, CMake,
Protobuf `protoc` and LLVM `libclang.dll`; set `LIBCLANG_PATH` to LLVM's `bin`.
Check that `where.exe link` resolves to MSVC, not Git for Windows' GNU linker.
Pull-request CI runs static web checks and three live-identity guards only.
Run tests needed for the changed behavior; do not repeat unaffected suites.
The full Ubuntu 24.04 / Windows 2025 matrix is a manual workflow option (`full`).

The default command runs frozen install, effective app/config typechecks, lint,
live-identity and native delay guards, navigation-order checks and preview isolation:

```sh
python scripts/verify.py
```

Local acceptance status and the separate cloud/local evidence matrix are recorded
in `docs/local-handoff/LOCAL_ACCEPTANCE_20261020.md` and
`docs/local-handoff/LOCAL_ACCEPTANCE_MATRIX.json`. Native Windows and real-model
results are reported separately from browser fixtures and WSL Linux checks.

Only when a full browser gate is explicitly needed, prepare the browser once
(the explicit install also supplies the headless shell):

```sh
cd modules/webcore
corepack pnpm install --frozen-lockfile
corepack pnpm exec playwright install --with-deps chromium
cd ../..
python scripts/verify.py --scope all
```

On Windows, `py -3.12 scripts/verify.py` also works. The opt-in full gate runs locked
workspace build/test/clippy, frozen web install, typecheck, lint, a production
build, the complete fixture E2E suite, and the real local-provider browser flow.
Every browser run uses one worker and zero retries. `-j 1` limits Cargo build
jobs; Rust tests keep their default concurrency. Do not set `RUST_TEST_THREADS`
for an acceptance run.

Each invocation creates a unique evidence directory with the source/lock hashes,
commit, tool versions, commands, exit codes and logs. The first failed command
stops the gate and returns its exit code. Never overwrite a failed first run:
use `--diagnostic` to label an investigation. After a fix, rerun the affected
checks only unless a full gate is explicitly required. A partial or diagnostic
pass must not be reported as a full-platform pass.

```sh
python scripts/verify.py --scope all --evidence-dir /path/to/evidence --temp-dir /path/to/tmp --target-dir /path/to/target
python scripts/verify.py --scope rust
python scripts/verify.py --scope web
python scripts/verify.py --scope live
```

Directory arguments work on both platforms; `CARGO_TARGET_DIR` is also honored.
`--scope live` requires the built binaries and production assets; set
`METTEUR_DAEMON_BIN`, `METTEUR_WEB_BIN`, and `METTEUR_WEB_DIST` for a nondefault
build location. The full gate builds these inputs itself. Component scopes are
useful for diagnosis and do not substitute for the full gate.

`corepack pnpm test:e2e` uses the deterministic mock gateway and an owned Vite
server. It fails on an occupied port instead of reusing another application;
`E2E_PORT` selects a different port. `E2E_BROWSER_CHANNEL=chromium` opts into the
full Chromium build; the default uses its separately installed headless shell.
`corepack pnpm test:monkey` remains an optional interaction sweep.

`corepack pnpm test:live` starts an independent workspace, daemon, production
web server and loopback HTTP provider. It verifies the production gateway
identity, a real file-tool round trip, and stored conversation restoration.
It never loads a user's model configuration or calls a paid API. Random ports
are the default; `METTEUR_PROVIDER_PORT`, `METTEUR_DAEMON_PORT` and
`METTEUR_WEB_PORT` can select ports explicitly. `METTEUR_VALIDATION_DIR` selects
the evidence/workspace parent. Only owned child processes are stopped. Logs,
synthetic data and provider receipts remain available for inspection.

To drive that owned fixture from a browser on another host, start
`node scripts/run-live.mjs --serve-only` in `modules/webcore`, explicitly bind
the intended interface with `METTEUR_BIND_HOST`, and use the generated
`ready.json` values for `LIVE_BASE_URL`, `LIVE_WORKSPACE`, `LIVE_FIXTURE_URL` and
`LIVE_NONCE` with `playwright.live.config.ts`. Publish container ports only on
loopback. This is a test fixture, not a deployment or a real-model preview.

The live test waits for initial deep-link navigation to settle. That wait does
not fix the existing initial navigation race. The workspace PID lock's generic
PID-reuse and abrupt-termination risks also remain separate recovery work;
never remove locks or reuse existing preview data to make a test pass.


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
