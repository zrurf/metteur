# NX-M5 local candidate — 2026-10-15

User deferred actual two-model adaptation/comparison (NX-M4) and authorized M5. M4 preparation remains at f75c7daf84c6db6e07e522e866b58af1ae108b41; its real-model acceptance remains NOT_RUN.

PV-06/r1 and PV-07/r1 were shown with concrete screenshots and provisionally approved by message Sentinel_106b3f6af6ec819191a269986e0ac706 at 11:46:54 UTC. Final all-new-pages review remains required. This is a local candidate, not upstream delivery. No M6, paid API, push or merge performed.

## NX-38 — implemented, targeted checks passed

Real Extism cancellation, per-call cancellation token, host-owned active-time budget excluding bounded human approval waits. Parent Stop or dropped async ownership signals the same blocking worker; no automatic replacement. Late approvals fail. Errors retain recorded file mutations and journal evidence.

Limits: HTTP may take its existing 10-second bound to return and may already have remote effects. OS filesystem calls cannot be forcibly stopped. A cancellation signal does not prove termination or absence of effects. No retry based solely on an expired lease. Dedicated slow-I/O failure injection was not run.

## NX-39 — implemented, targeted checks passed

Signed manifest-declared self-contained HTML pages, immutable package bytes, bounded assets, subject/workspace/package/page/nonce lease binding, strict method and parameter schema, replay rejection, rate/count limits, revocation on package reconciliation and workspace close. One bounded AddonPages RPC; page receives a MessageChannel, never an RPC client or credentials.

Opaque sandboxed srcdoc iframe, default-deny page CSP and parent frame-src 'none'. Actual Chromium probe confirmed srcdoc script works and navigation to the host app is blocked with effectiveDirective=frame-src. No full adversarial matrix or cross-browser claim.

## NX-40 — provisionally approved UI integrated

Addon pages has its own workspace route, activity entry and closable tab. Correct sidebar ownership, signed page entry, capability details, empty/loading/error/revoked states. Local browser opened the actual signed sample and verified that disabling it removes the old iframe and stops access.

## NX-41 — real source-diagnostic loop implemented

Registered local target → explicit host selection → bounded snapshot/digest → actual source diagnostics → append-only report → independent host-owned human acceptance. Changed content/revision rejects stale acceptance. The addon cannot issue acceptance or permission decisions. Local fixture browser acceptance is test evidence, not the user's final acceptance of the product.

The supplied checker is deliberately source-only: UTF-8, doctype, title and viewport declarations. Screenshot, browser console, rendered-layout checks and model opinion remain null/not_run. This satisfies the diagnostic variant of the sample; it is not a complete visual testing engine. Saved reports survive in the daemon's addon-state/inspection-reports directory; reopening a page starts a new lease and does not pretend to restore an old active session.

## Validation summary

Six distinct targeted Rust tests passed (three new execution-control tests, one existing file-host regression, one page-boundary test, one inspection/review test). Vue typecheck, focused lint, production web build and Linux daemon/web-server build passed. Actual browser checked normal inspection/acceptance and disable revocation. Historical failures retained: Wasmtime cache outside writable workspace, test include path and digest formatting; all corrected before the passing reruns.

Remaining: final visual review and refinements; deferred M4 real-model work; M6 necessary acceptance, recovery/migration scope and final delivery audit. No full suite, full E2E or platform matrix is implied.
