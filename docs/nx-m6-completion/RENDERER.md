> 2026-10-21 当前补充：受控真实模型意见与签名安装页验收已完成，见 [本地收尾](../local-handoff/COMPLETION_20261021.md)。下文旧 not_run/待办为历史阶段状态。

# Offline preview renderer

This is an optional operator-owned host adapter. It renders only the exact selected registered local HTML bytes. It is not a general browser agent, does not visit an arbitrary URL and makes no paid/model requests.

Host configuration (absolute paths, no credentials):
- METTEUR_PREVIEW_NODE: installed Node 24+ executable.
- METTEUR_PREVIEW_RENDERER: modules/webcore/scripts/preview-inspection/render.mjs from this trusted source checkout.
- METTEUR_PREVIEW_CHROMIUM: optional installed Chromium executable; otherwise use the already-installed Playwright Chromium.

The repository's locked web dependencies must already be installed. There is no automatic package/browser install, no disabling of the Chromium sandbox, no personal browser profile. The daemon does not accept these paths from the plugin or from the workspace preview registry.

The worker receives only a max-256-KiB snapshot and digest on stdin. Fresh browser context, no credentials, offline, all routes denied except one synthetic response containing those bytes. CSP arrives as an HTTP response header (including sandbox without same-origin, no frames, workers, forms, connections or base URL). Popups/dialogs/downloads are blocked or closed. Snapshot settling budget 250 ms, 1280x800 viewport, 100 bounded console entries, 1.5-MB screenshot, total worker deadline 15 seconds, one concurrent worker. Evidence is checked against SHA-256 and exact target revision again before append-only persistence. Closing/disabling/changing the package revokes the report attachment even if a worker was already running.

An incomplete screenshot, blocked external assets, absent API data or an empty console is not full visual/accessibility acceptance. Report fields disclose the isolation scope. Human acceptance remains explicit in the host; model opinion is still not_run.

Without configuration, source checks remain available and browser_checks=not_configured. Missing browser/sandbox errors are failed, never successful checks. A busy worker returns busy. The plugin cannot interpret either as a rendered pass.
