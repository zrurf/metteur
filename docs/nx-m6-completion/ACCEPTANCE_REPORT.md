> 2026-10-21 当前补充：受控真实模型意见与签名安装页验收已完成，见 [本地收尾](../local-handoff/COMPLETION_20261021.md)。下文旧 not_run/待办为历史阶段状态。

> 最新本地交接说明（2026-10-15）：以下为阶段历史记录。当前状态、类型检查更正和后续全部任务以 `docs/local-handoff/START_HERE.md` 为入口；历史“前端typecheck通过”不能当作有效app检查通过，当前尚有33条既有诊断。规模/渲染等后续补齐以最新报告为准。

# M6 non-model completion report

Source implementation: 739cf6a743b969df468d3aa225f3b90f74b3d069. Prior release 1147780 remains preserved. No paid model calls, remote push or merge.

## Completed additions
- Offline Chromium renderer behind the existing selected-preview bridge: digest/revision-bound screenshot, bounded console, overflow/alt-attribute diagnostics, append-only report, fail/busy/not-configured statuses. Concrete PV07/r2 preview presented and continuation approved before sample-page integration. Model opinion remains not_run.
- Async network transfer inside the synchronous Wasm host callback. Stop and call timeout cancel local response waiting, including a stalled response body. They do not assert external effects were undone. File operations reject non-regular targets and check cancellation after bounded reads.
- Kernel-owned workspace lock replaces non-atomic PID-file ownership; stable inode, explicit unlock, process-exit recovery and legacy live-owner upgrade guard. Do not run legacy and new daemons against the same workspace.
- Native blueprint source can be loaded without a surviving old database mirror. Corrupt sources now surface an error instead of silently opening a new empty graph.
- Initial route readiness precedes automatic workspace restoration. Large graphs render only visible elements; all model nodes/edges remain intact.
- Shared 16-MiB RPC envelope budget accommodates the measured ~7-MiB checkpoint, replacing mismatched implicit 4-MiB client limits. Smaller endpoint limits remain enforced. This is a bounded tested envelope, not unlimited history paging.
- Local controllable model fixture ends after a declined blueprint launch instead of repeating it. Actual browser receipts: one proposal plus one final response, zero confirmed execution.

## Necessary evidence
- Lock group: 4 substantive checks + one child-process fixture entry passed. Includes simultaneous contenders, stale/reused PID marker and real abrupt child termination/reopen. Initial parallel test exposed inherited-descriptor release timing; explicit unlock fixed it. Initial failure retained.
- Slow HTTP: one case covers Stop and active timeout; final observed returns ~15 ms and ~259 ms after stalled-response notification; subsequent file mutation absent. Parent cancellation remains false for a local timeout.
- Source/report/revision sample and forged renderer evidence checks passed. Existing signed-page/revoke security semantics retained.
- Host renderer: actual normal Addon page Run checks -> real Chromium -> persisted report completed, human acceptance null. Boundary fixture executes known local script, captures console and overflow, and local network leak detector receives 0 requests. Not complete attack certification.
- Scale: actual Blueprint validation with 1,000 nodes/999 edges; ExecutionView production methods build 10,000 invocation records; 7,155,148-byte durable checkpoint preserved through 12 actual database reopen cycles. Validation 132 ms, record construction 10,759 ms, write 303 ms, reopen total 2,798 ms in this environment. Synthetic records, not 10,000 paid model calls or a wall-clock soak.
- Actual browser graph: 1,000 nodes/999 edges initially mounted; same viewport after culling mounts 20 nodes/25 edges, keeps graph intact. File-import failure seen before fix and retained.
- 7.2-MB backend -> proxy -> client transport probe passed in 44 ms; permission-denied path still denied. Existing test backend lacked 14 new resource RPC stubs, compile failure retained and fixed (unimplemented stubs, not fake successes).
- Existing bounded event probe re-run: 64 sources, 4,096 duplicate events, 8 recoveries; 64 terminal events remain identical, stop generation unchanged.
- Frontend typecheck and production build passed; Linux daemon/web build passed. No full suite, no full E2E or platform matrix.

## Boundaries that require external action
1. Real-model selection, provider-specific adaptation, matched live evaluation and model review remain deferred.
2. Native Windows has not been run. A connected desktop was discovered, but current user instructions require the user to send any local Codex instructions themselves. scripts/nx-windows-targeted.ps1 is prepared; no local task was created.
3. Final visual acceptance belongs to the user. Screenshot guide is for review, not a substituted approval.
4. Remote publication is still paused; no fork/origin push, PR update or merge.
5. Bounded repeated recovery is not a multi-day soak. Arbitrary kernel/filesystem stalls cannot be forcibly rolled back. The renderer supports self-contained local HTML, not arbitrary authenticated web applications.

## Preservation
Original handoff, earlier candidate bundles, first failures and previous review records remain. New candidates do not inherit human acceptance. Supplement package contains updated readable source, Git bundle, docs, selected logs, screenshots and the PDF guide.

## Final browser review additions
- A single-turn local fixture continuation completed with 80 reported tokens; no real API used, no outstanding automatic turn. Schedule form inspected in Once and Weekly states; no schedule created when no saved waiting task exists.
- Current project/personal Prompt states, edit and convention dialogs reviewed without promoting/deleting user data.
- New Addon presentation integrated from presented PV07/r2. Synthetic demonstration HTML is only a renderer input; no business metrics are claimed.

Final actual host report: browser_checks=completed; 4 source + 2 isolated-browser diagnostics; 1280x800 screenshot with digest; known synthetic console entry; human_acceptance=null. Source checks, browser facts, unrun model opinion and user acceptance remain distinct.
