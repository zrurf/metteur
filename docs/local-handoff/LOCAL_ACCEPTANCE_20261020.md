> 当前收尾结果（2026-10-21）：[COMPLETION_20261021.md](COMPLETION_20261021.md)。下文为接收/历史过程记录，旧的未付费、未测 Windows、未完成配对状态已被当前证据取代。

# Local acceptance in progress — started 2026-10-20, updated 2026-10-21

> Later model/configuration audit on 2026-10-21: see [UNFINISHED_AUDIT_20261021.md](UNFINISHED_AUDIT_20261021.md). Existing DeepSeek configuration verified by two bounded real generation requests under the owner's USD 5 synthetic-data authorization. Historical zero-call statements below describe the earlier engineering candidate; no paired evaluation has run.

Base: `ff7b795e536042e885f08a9bf7a2e5d4a3121749`. The owner requested local tests, comparison with the complete NX plan, repairs and quality improvements. Active branch: `codex/local-acceptance-20261020`. The original M1 checkout and downloaded package are preserved.

## Frontend gate repaired

- Reproduced all 33 effective application diagnostics. The former references-only root command did not establish application type safety.
- `pnpm typecheck` now checks the application and every root `*.config.ts`. Strict source checking stays enabled. Removed deprecated `baseUrl`; relative alias paths replace it. ES2023 library declarations cover the existing `findLast` calls without changing the ES2022 build target.
- Unified CodeMirror state/view at the already locked direct versions; frozen offline installation validated the targeted lockfile change. No general dependency upgrade.
- Repaired shared context/config types, message-role inference, reasoning defaults, Markdown renderer types, unused imports and an unreachable comparison. Concierge copying now runs in script and handles clipboard failure.
- Node checking skips third-party declarations, matching the existing Vue app configuration. This avoids unused optional PWA generator/worker declarations while checking all owned configs under `strict`; application diagnostics were repaired in source.
- Negative coverage probes inserted an invalid assignment separately in an app file and a config. The ordinary command rejected both with TS2322; probe files were removed.
- Fixed a mock Job defect: Kill could be followed by more output and a successful completion. A browser regression verifies one killed terminal notice, no later output and retained killed state.

Evidence parent: `D:/BaiduNetdiskDownload/metteur-a7-work/tmp/local-acceptance-20261020/`.

| Check | Result | Evidence directory |
|---|---|---|
| Baseline effective app types | Failed: 33 diagnostics | `baseline-app-types-omafq6xx` |
| Baseline Node config types | Failed: 17 third-party declaration diagnostics | `baseline-node-types-ihudqmrf` |
| Repaired app + config types | Passed | `repaired-types-18jq5b69` |
| Negative coverage probes | Both expected errors detected | `typecheck-coverage-directed-8wh7l5ua` |
| Lint | Passed | `fixed-web-lint-mvoav7dp` |
| Production frontend build | Passed; large-chunk warnings retained | `fixed-web-build-se3wyzo0` |
| Mock gateway browser suite | **94 passed**, 1 worker, 0 retries | `fixture-e2e-owned-port-5idnbprw` |

The first browser attempt failed before tests: Windows denied the chosen localhost/IPv6 port. The passing run bound IPv4 loopback and used an OS-assigned available port, without reusing another app's server. An initial coverage probe overlapped pnpm dependency recreation and failed as an environment error; `typecheck-coverage-bbznjlrb` is preserved and not counted as a coverage pass.

## Remaining acceptance

- Native Windows initially failed: a minimal program using installed Rust 1.99.0 returned `link.exe not found` (`native-linker-probe-7wf65372`). The owner subsequently requested installation. MSVC/SDK/CMake and LLVM are now installed and the functional tool probes pass; project-level native acceptance remains in progress.
- Cargo offline preflight could not resolve cached `anyhow`; no tests compiled (`native-shared-preflight-icmhm95q`).
- Local WSL Ubuntu 24.04 build prerequisites and exact Rust 1.99.0 are installed. The initial locked workspace build passed (`linux-workspace-build-l8jyjnig`); this is separate from native Windows validation.
- Paid model calls are permitted in principle, pending the owner's exact models, endpoints, data scope and spending limit. No real-model calls have been made.
- Mock browser tests do not replace real daemon/provider tests or cover all new NX surfaces. Backend integration, resource/task/schedule interactions, the 45-task matrix and final human visual review remain open.
- No remote publication, PR update or merge.

## Resource ownership and draft regressions

Additional directed probes found defects not covered by the former suite:

- A pending selection from workspace A survived A → B → A and repopulated cleared selections. Workspace epochs now synchronously invalidate old requests, including refreshes.
- Replacing the selection list during validation returned true for a new unchecked fragment. Validation now binds the exact items, and Chat also verifies the same snapshot IDs before sending.
- A late Project save acknowledgement closed a newer Personal edit. Save completion now binds the scope/workspace view and edit session.
- Filtering the resource list while editing sent the old body to the newly visible resource ID. The editor now retains its original resource/revision independently of filtering.
- Saving prevents duplicate submissions and preserves text typed after submission as a newer unsaved draft.

Initial failing evidence: `resource-races-before-fix-qtg1c3k4`, `resource-save-before-fix-tr0r3rt_`, and `resource-draft-binding-before-fix-kxnyoirb`. The first scope-save attempt also exposed an incorrect summary-button test locator; that locator was repaired before the actual draft-loss failure was recorded.

The repaired five resource cases plus affected Chat/composer/rewind/blueprint-save tests passed: **33 passed**, one worker, zero retries (`resource-chat-blueprint-regression-9ntxd8fk`). These are 28 reused regression cases and five new cases, not 33 additional distinct requirements.

This remains a mock gateway/browser check. Actual daemon ownership/permission checks and provider behavior remain separate acceptance layers.

## Further local verification work

- Marked the Blueprint custom node component raw and reused its type map. The four save-failure browser cases now also assert that rendering emits no reactive-component warning; all passed in the 33-case directed run.
- Pinned `verifyDepsBeforeRun: error`: scripts fail on stale dependencies instead of silently reinstalling them during other checks. An overlapping pnpm install previously caused an EPERM rename failure (`resource-race-types-eqv0gcd6`); this environment failure remains recorded. After explicit frozen installation, normal app/config checking passed (`current-frontend-types-xk9gxphl`) and lint passed (`current-frontend-lint-fhvpf6ch`).
- Replaced the NX fixture's `sleep 8` command with an actual Node timer and ensured its daemon inherits the launcher's Node directory in PATH. Native Windows shell execution and PATH handling passed four unit cases; identity checks passed three more (`fixture-identity-and-native-delay-l4urco9k`). This proves the command helper, not the complete native daemon flow.
- Navigation normalization/move/upgrade assertions passed (`navigation-order-jqkxezq4`); preview registry/production-entry isolation passed (`preview-isolation-e_qqnstl`). These checks are now included in the ordinary verification script.
- `LOCAL_ACCEPTANCE_MATRIX.json` enumerates all **45 NX tasks and 88 scenarios**, with unrun and partial local states retained. Model authorization is pending concrete configuration and budget; native Windows project validation is in progress.

## Project memory concurrency repairs

Three further directed browser probes reproduced source evidence appearing under a different note, filtering retargeting a draft to another note, and save acknowledgements discarding newer text (`memory-races-before-z4vx1zn1`: three semantic failures).

The memory editor now retains its original note and revision, invalidates source requests on selection/workspace changes, and binds save completion to its edit session. Newer text remains unsaved after the submitted revision is acknowledged. Workspace changes invalidate mutation completions synchronously.

All three probes passed after repair (`memory-races-fixed-666mybqp`), with effective app/config types (`memory-and-layout-types-m6bo3zjj`), lint (`memory-resource-lint-o1fcb7b9`) and production build (`frontend-production-memory-wszd5ubn`) passing. Native daemon memory validation remains separate.

A further paging/reload probe reproduced an edited note disappearing from the editor when the latest returned page omitted it (`memory-page-before-w3xnqwj1`). The editor now renders its retained source snapshot independently of the returned page and advances its base revision directly from the write response. The four memory cases passed (`memory-snapshot-revision-hnipyak_`), and an additional directed check confirmed an off-page draft survives acknowledgement and sends revision 2 on its next save (`memory-page-revision-chkk5zw5`). The updated ordinary gate also passed frozen install, app/config types, lint, seven native/identity guards, navigation checks and preview isolation (`web-final-gate/first-k44as6eq`), with the recorded source digest unchanged during that run.

The latest frontend production build also passed (`frontend-production-current-69yf_zgd`); its output is `D:/BaiduNetdiskDownload/metteur-a7-work/build/local-acceptance-20261020-web-current`. This artifact includes the final memory paging and launch-review fixes. Its large-chunk and EditorPane import warnings remain disclosed.

The ordinary verification script also passed all six command steps (`default-gate/first-8umb2qrd`). A later direct production command detected a pnpm CI/non-CI virtual-store difference before building (`frontend-production-r2-mb78przy`); the workspace now explicitly selects the same local store layout in both modes. Typechecking and the latest production build passed in a normal shell without setting CI. Dependency versions did not change.

## Native Windows tools installed and exercised

The Microsoft-signed Build Tools installer installed Visual Studio Build Tools 2026 18.11.0, MSVC 14.51, Windows SDK 10.0.26100.0 and CMake 4.3.1 at `D:/Tocoding/VSBuildTools2026`. The official LLVM 23.1.3 x64 MSI was matched to the winget manifest SHA256 `5153f848ac87553118340f16f8d3cd4351d729bcba6e4785c6a4a9af29676de9` before installation at `D:/Tocoding/LLVM`. Both installers returned 0 and VS reports no reboot required. Installation logs are retained in the workspace review's `native-tools-install` directory.

Actual unprivileged probes passed (`native-tools-functional-f3p3rtvp`): Rust 1.99.0 linked and ran a native executable, MSVC compiled and ran a C++ executable using Windows SDK APIs, and Python loaded libclang.dll and queried its version. CMake and protoc executables were also exercised. Two earlier probe attempts failed in the test launcher's quoting/PATH resolution before compilation; both are retained (`native-installed-tool-probes-wzh3h0m1`, `native-installed-tool-probes-fixed-08y7yfqx`) and the launcher was corrected. These probes establish tool availability, not a project test pass.

The project's shared-module native Windows unit suite subsequently passed: **57 passed, 0 failed, 0 ignored** (`native-shared-tests-pse0g8zb`), with default test concurrency and a single compiler job. The separate daemon lock/storage/recovery cases are next; shared tests do not substitute for them.

The first native targeted-script attempt stopped before testing because Windows PowerShell 5 converted normal Cargo stderr progress into a terminating ErrorRecord (`native-daemon-targeted-5l3z16g9`). The script now captures native streams directly and checks the actual process exit code. A labelled native stub probe verifies both successful stderr handling and failure propagation: exit 7 stops later groups and produces no success marker (`windows-gate-contract-1ewoepf1`). This validates the script contract only. Actual daemon cases were restarted separately (`native-daemon-targeted-fixed-qm39jt7o`).

## Offline renderer worker exercised

Four native Windows worker checks passed (`native-offline-renderer-_3bhcros`): an exact-digest screenshot and console capture, blocked external fetch/image/frame and opaque-origin storage, rejection of a wrong digest, and rejection of an oversized snapshot. An owned loopback sentinel received **zero** requests. Chromium's sandbox stayed enabled; no user browser profile was used. The generated 1280×800 JPEG was inspected. This proves the shipped worker with synthetic snapshots; daemon adapter attachment/revocation and final user acceptance are separate.

## Blueprint launch review identity repaired

Two directed browser probes reproduced stale approval state when a proposal changed under the same request ID and an old response resolving a replacement request (`launch-review-before-qso4yc5d`). Review confirmation now binds the complete proposal, request and workspace; changes synchronously reset confirmation and invalidate old callbacks. Unmounting also invalidates pending responses.

Four directed cases passed (`launch-review-binding-g5fpfc5b`): changed proposal clears approval, old acknowledgement does not resolve a new request, policy changes require fresh approval and send the exact digest/scope/policy, and Decline records a negative decision without requiring approval. Effective types passed (`launch-review-types-iju5zaf8`), lint passed (`launch-review-lint-w4cf_zaz`) and the updated production build passed (`frontend-production-launch-8gde207w`). Existing EditorPane/chunk-size warnings remain. These are browser component fixtures; actual host version checks remain a separate layer.


## Actual daemon acceptance and native checkpoint repair

The complete workspace builds on both native Windows (`native-workspace-build-zeqx_w2j`, exit 0) and WSL Linux (`linux-continuation-build-srs0ixhz`, exit 0). The following evidence uses the repaired source, with the dirty source list retained by the launcher; a clean frozen final Rust gate is still required.

The first native daemon targeted run passed all five lock tests and the additive migration/CAS/replay/restart/rollback test, but failed the recovery admission test (`native-daemon-targeted-fixed-qm39jt7o`). Diagnosis exposed an actual Windows path defect: RocksDB appends forward slashes while creating a checkpoint, which fails for canonical verbatim-prefix paths. Database open already normalized such paths, but checkpoint creation did not. Checkpoints now use the same normalization. The new canonical-path regression proves the checkpoint retains the old value after the live database changes (`native-canonical-checkpoint-oalhtm6t`: 1 passed). Both recovery admission cases then passed (`native-recovery-fixed-i59ki_j4`: 2 passed), including corrupt saved chat state preventing the turn before any file write. No test assertion was relaxed.

Actual production browser → owned daemon → isolated synthetic provider checks passed:

- ReadFile tool round trip and conversation persistence across reload (`linux-daemon-live-chat-f44ftqie`; repeated on the repaired candidate in `linux-live-chat-v2-sualv4f3`).
- A real eight-second Job completes, the host dispatches exactly one continuation and its result reaches the original session. Reservations return to zero; declining a blueprint starts no execution and produces no repeated proposal (`linux-daemon-live-nx-transition-4wdpse3g`).
- Explicit blueprint version/scope/policy approval starts exactly one execution, produces one terminal dispatch and completes its parent task (`linux-live-blueprint-follow-ooeig6gk`).
- Reloading the browser after the chat safely yields restores the saved task identity and receives the completion in the original session (`linux-live-blueprint-reload-_moki4dd`).

The positive blueprint probe first found another actual defect: the daemon completed and persisted the answer, but the current chat surface stayed at its waiting notice (`linux-live-blueprint-completion-1hm4er1t`). A structured host task ID now travels in the live event and persisted transcript. The chat performs scoped read-only status/session refreshes after yielding, invalidates late responses on conversation/workspace changes and preserves pending composer files. It sends no model request and never controls or resumes a task. Paused/stopped/attention states use the saved host state. The three new browser regressions and affected chat/rewind/resource/launch tests passed: 42 passed, zero retries (`chat-following-regressions-gyfbusp6`). The latest ordinary web gate passed all six command steps with unchanged source digest (`web-follow-gate/first-bbohctr8`); latest production output is `D:/BaiduNetdiskDownload/metteur-a7-work/build/local-acceptance-20261020-web-follow` (`frontend-production-follow-g67zzp09`).

All provider traffic above used a nonce-verified loopback synthetic service. No real-model or paid request has been sent. Setup-only browser locator failures remain recorded separately; the earlier stale-UI failure is a product defect and is retained as failing evidence.

The earlier scoped Linux frozen Rust attempt was deliberately interrupted before tests to repair the two actual defects (`linux-frozen-rust-gate-scoped-git-4ayc_vwl`, exit 2); it is not a passed gate. A global stable rustfmt check also reports substantial existing cloud formatting differences and nightly-only configuration options; no broad formatting rewrite was applied. Final full tests/clippy, the clean native targeted rerun, remaining plan scenarios, real models, scale/long-run drills and owner visual approval are still open.

The first live stack was stopped and its daemon/web child PIDs verified absent before the retained closed-stack archive was made (`linux-owned-live-stack-f315b24s/closed-stack-evidence.tar.gz`, SHA256 `80c0e8437ee130e7761f56ce89bb1c19a02ef0492f2905500f492e031c7e4b38`). An earlier running snapshot is explicitly named `unverified-live-diagnostic.tar.gz` and is not recovery evidence.


## 2026-10-21 current candidate update

This section supersedes the earlier in-progress platform statements. Native Windows tools, full workspace build and production daemon/browser flows are verified. The clean full Linux attempt reached 725 reported passing tests and one failure, with zero ignored tests in the programs reached (`linux-frozen-rust-final-sjl43_2m`; detailed `linux-frozen-gate/first-hl0x9tdw`). It stopped at the smoke program and did not run later programs or clippy. Counts include Rust helper entry points and are not distinct requirements.

The failure was the frontend demo node catalogue missing the already implemented `PromptResource` node. Actual native daemon `ListNodeKinds` RPC data confirmed 67 → 68 kinds, one added kind, zero removals and zero changed existing signatures (`node-catalog-native-rpc-ia1wrslw`). The fixture now includes the node and its string output pin. The equality assertion remains unchanged. An attempted filtered Cargo regeneration was interrupted during dependency recompilation, before executing the test (`node-catalog-regenerate-m75w_4es`); trying its removed old test binary failed before execution (`node-catalog-built-regeneration-2ysy7led`). Neither is counted as a test pass. A new complete run is pending.

The native targeted script exposed a second PowerShell behavior: `Start-Process -Wait` did not return after Cargo and its tests exited (`native-final-targeted-_3gdz7wf`, intentionally stopped; process identity retained). It now retains the Cargo process handle and waits for that process, preserving its actual exit code. The Microsoft documentation describes the old parameter as waiting for the entire descendant tree: https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.management/start-process?view=powershell-5.1. The exact lingering descendant was not established. A direct-wait probe without retaining the handle correctly failed on unavailable exit code (`native-direct-process-wait-at16vh9l`). The repaired candidate passes successful-stderr and exit-7 contract probes (`windows-handle-contract-8m3wxl97`) and all three actual native groups (`native-targeted-serial-w80j_8tn`): lock 5, storage 1, recovery 1. A previous attempt was blocked by the owned live daemon holding its executable open (`native-owned-handle-wait-30rpqg9o`); services were stopped before the passing serial run. This is an environment failure, not a hidden test pass.

Native Windows production checks also passed: real file-tool chat/reload (`native-live-chat-r88c2gc5`), actual Job completion/one host continuation and blueprint decline (`native-live-job-decline-ouk3k220`), and one approved blueprint completing in the original chat after reloading during its safe yield (`native-live-blueprint-reload-6xt0_jkd`). The isolated workspace was closed, daemon restarted and its original two task IDs compared unchanged (`node-catalog-native-rpc-ia1wrslw`). All model traffic was synthetic loopback traffic.

Actual page review found a setup-time temporal-dead-zone error: TaskPanel registered a watch source that accessed `entry` before its computed declaration (`native-live-page-review-labels-ol1zheqw`). The declaration now precedes the watch. The new browser regression opens the bounded task dialog and asserts both page and console errors are absent (`task-panel-initialization-fixed-fptmq6m6`: 1 passed). Its first mock setup attempt left the gateway disconnected and therefore correctly kept New task disabled (`task-panel-initialization-678bjxsf`); that setup-only failure is retained.

The repaired native production frontend passed 28 rendering checks across seven surfaces, light/dark themes and 1440/960-pixel windows, with no document overflow, service alerts, page errors or console errors (`native-live-page-review-ready-99wj4s_e`). Screenshots are in `native-surface-browser-PT6ZC7`. Actual keyboard ordering, pointer drag, persistence after reload and reset also passed in that run. These are technical checks, not owner visual approval. Earlier helper attempts used an exact heading locator that included hidden help, then an inappropriate network-idle wait despite persistent streams; those setup failures remain recorded separately. One static-copy quoting failure left the prior artifact in place and was corrected before the passing run.

Effective types passed (`task-catalog-types-jh_vk8l1`), lint passed (`task-catalog-lint-ycfleudg`), and the latest production build passed (`frontend-production-final-g_uv1tva`): `D:/BaiduNetdiskDownload/metteur-a7-work/build/local-acceptance-20261021-web-final`. Large chunks and the existing EditorPane import warning remain. The ordinary checks gate has six command steps; seven refers to its native/identity guard test cases, not the number of commands.

Still pending: a complete passing Rust test/clippy run after catalogue repair, source-script native confirmation, real two-model configuration/budget/data scope and paired evaluations (6 development/12 held-out seeds remain specifications), final owner visual approval and local package recovery verification. No remote publication is authorized or performed.

The subsequent full Linux run passed the original catalogue equality check and all daemon programs, then stopped at `metteur-harness` assembly (`linux-frozen-rust-repaired-662wwj0t`; `linux-frozen-gate/first-pwgcqf43`). Its exact-order test still expected eight sections and omitted the runtime contract added in cloud commit `f75c7da`, whose deliberate priority 85 places it between tools and coding. The expected list now explicitly includes that section; production prompt content/order is unchanged and the exact equality assertion is retained. Clippy was not reached in that failed attempt. The committed source-script Windows rerun passed all 7 reported cases (`native-source-targeted-final-y9byfft_`), and final frontend checks passed six steps (`frontend-source-checks-final-cbkx9hjz`). Catalogue/Addon-node/blueprint-authority browser regressions passed 7 cases (`catalog-affected-browser-nc1isopz`).

The next complete Rust attempt passed all 911 reported tests, with 0 failed and 0 ignored, but clippy stopped with `unused_io_amount` in the Addon slow-HTTP test (`linux-frozen-rust-final-candidate-t64j4b36`; `linux-frozen-gate/first-_ps1dkt2`). Its raw socket fixture discarded the read count. The fixture now reads complete request headers through a buffered reader, checks EOF/header/body bounds, and consumes the declared body before acknowledging the slow response. Stop/timeout/no-subsequent-mutation assertions are unchanged. This is a fixture protocol repair; production Addon behavior is unchanged. Clippy warnings are retained rather than globally suppressed. A final gate on the repaired fixture remains pending.


## Final local engineering gates — 2026-10-21

The repaired source candidate `3f8513ea51b9d35498908fca4ce37f1d94b18db9` passed the complete locked workspace **build, test and clippy --all-targets** gate on WSL Linux (`linux-frozen-rust-http-fixed-5tpc57gm`; detailed `linux-frozen-gate/first-21szndqv`). Source SHA256 remained `825936cb4a109b4c1af7f021b36a90ef7930ad9150bf73f119ca5ffd2850bb57` throughout the run. Default test concurrency, one compiler job and zero retries were used. Reported test totals: **911 passed, 0 failed, 0 ignored**; helper/subprocess entry points are included and are not separate requirement counts. The catalogue equality test passed with its original assertion. `test-summary.json` retains every reported program/case. Clippy exited 0; 29 warning headings remain in its raw log and are not represented as a warning-free result. Existing stable rustfmt differences remain outside this passed gate.

The committed Windows targeted script passed all three groups, 7 reported cases (`native-source-targeted-final-y9byfft_`), with services stopped before compilation. Native shared 57-case, canonical-checkpoint and two-case recovery results remain separately recorded. The final ordinary frontend checks passed all six command steps with unchanged source digest (`frontend-source-checks-final-cbkx9hjz`, `web-source-final-gate/first-n3gsr_v3`). The affected catalogue/addon-node/blueprint-authority browser run passed 7 cases (`catalog-affected-browser-nc1isopz`). These directed reruns do not add to the original 94 as distinct tests.

The original M1 checkout remains clean at `2a365e083ca6d3f6cde18630fc8d9507a3d69f64`. Owned demonstration workspaces were closed before stopping their services; no paid model call was made. Native fixture raw exit 1 records the operator stopping its owned web child, after which it cleaned up daemon/provider; separate functional checks retain exit 0. The closed native synthetic-data archive receipt explains this shutdown (`native-reopened-live-rof7mqwr/closed-synthetic-data.receipt.json`).

The 45-task/88-scenario matrix now links scoped local backend cases, native/live UI evidence and explicit boundaries. Partial coverage is not promoted to complete scenario acceptance. The finite scale test ran as part of the ordinary Rust suite; no separate long stress campaign or multi-day soak was started.

Remaining work is explicit: the owner supplies exact two-model IDs/interfaces/endpoints, permitted data scope and a spending cap; the agent then prepares/freezes the runnable 6-development/12-held-out seeds, performs matched real-model comparisons and evidence-based role adaptation, and completes real-model Addon opinion checks. Final human visual approval belongs to the owner; 28 current technical screenshots and the actual approval/decline flow are ready for review. Other untested or partial scenario details remain in the matrix. Publication has not been requested. A new local handoff export separately records its exact HEAD, all-file digests and clean bundle recovery; see its STATE.json and recovery logs.
