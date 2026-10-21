> 2026-10-21 当前补充：受控真实模型意见与签名安装页验收已完成，见 [本地收尾](../local-handoff/COMPLETION_20261021.md)。下文旧 not_run/待办为历史阶段状态。

> 最新本地交接说明（2026-10-15）：以下为阶段历史记录。当前状态、类型检查更正和后续全部任务以 `docs/local-handoff/START_HERE.md` 为入口；历史“前端typecheck通过”不能当作有效app检查通过，当前尚有33条既有诊断。规模/渲染等后续补齐以最新报告为准。

# Continuation boundaries

The cloud-local non-model implementation is at 739cf6a; subsequent commits add a portable local-fixture entrypoint and delivery documentation. The 24-page Chinese PDF is the complete new-page screenshot/operation review guide (seven preview scopes, their dialogs, actual renderer output and bounded evidence).

Next actions requiring the owner:
1. Review the PDF and give final visual feedback by page number. Provisional integration approval is not final visual approval.
2. Run scripts/nx-windows-targeted.ps1 in the native Windows source workspace with an absolute NEW output folder. No install or provider calls are performed by that script. Preserve any existing local changes first; return the output folder. Do not infer native Windows acceptance from Linux.
3. Decide when to resume deferred actual-model work and supply current authorized model configuration separately; no credentials are in the handoff.
4. Decide publication separately. No remote push, PR changes or merge has occurred.

Do not erase old candidates, failure logs, original files or annotations. Read the active approval rules and environment instructions before issuing any execution requests. Do not treat this handoff as permission to change credentials, external access or publish code.

For runtime reproduction, build the documented Linux executables and web distribution with the locked toolchains. The optional preview worker requires existing Node 24+, web dependencies and Chromium. See RENDERER.md; never disable the browser sandbox to make a check pass. Without the optional worker, source checks still work and rendered status is explicit.

Portable local fixture: after a locked build, run `node modules/webcore/scripts/run-nx-local.mjs --serve-only`. It creates an isolated workspace, registers the saved local review graph and uses only the local controllable provider. A fresh chat with `M3_LOCAL_BLUEPRINT` presents a launch for explicit approval/decline; refusal closes the fixture turn. This entrypoint does not run the full E2E suite. Its path normalization/syntax is checked; observed live behavior used the same cloud harness and is not relabelled as a second standalone end-to-end run.
