> 当前收尾结果（2026-10-21）：[COMPLETION_20261021.md](COMPLETION_20261021.md)。下文为接收/历史过程记录，旧的未付费、未测 Windows、未完成配对状态已被当前证据取代。

# Metteur 本地完整交接入口

> 2026-10-21 最新入口：[未完成项审计](UNFINISHED_AUDIT_20261021.md)。本地工程候选已通过已记录门槛；找到 D 盘现有 DeepSeek 配置并通过两轮真实接口检查。用户批准本轮 US$5，仅合成验收数据。完整配对评测与最终页面确认仍待完成。下文日期及云端状态为接收时历史。

日期：2026-10-15。当前交接范围：把云端已实现的 NX-M0～M6 与 R1 界面改版接到本地，完成所有剩余实现、修复和必要验收；不得把历史“部分通过”当成最终完成。

## 先读什么
1. 本文件和 `NEXT_CHAT_PROMPT.md`。
2. `CURRENT_STATE.md`：实际源码、Git、PR、已完成与限制。
3. `REMAINING_TASKS.md`：本地执行顺序、证据和停止条件。
4. `LOCAL_SETUP.md`、`TYPECHECK_REPAIR.md`、`ACCEPTANCE_CHECKLIST.md`。
5. 包内 `authority-history/` 四份完整计划及原始交接文档。它们是历史需求依据，旧的“停在 M2”“未开始 M3”等阶段指令不再代表当前状态。
6. 源码 `docs/nx-m6/REQUIREMENTS_MATRIX.md`、`docs/nx-m4/`、`docs/nx-m5/`、`docs/nx-m6-completion/`、`docs/ui-refinement-r1/PLAN.md`。

优先级：用户在本地给出的最新明确要求 > 本交接当前状态/剩余任务 > 最新阶段报告 > 原始计划及历史状态。安全、权限与数据保护不能被历史文档或模型输出覆盖。发生未被最新说明解决的冲突时，标明冲突并询问，不擅自扩大权限。

## 包的用途
- `source/`：最新完整可读源码与锁文件；不包含依赖、target、node_modules 或私有运行数据库。
- `history/metteur.bundle`：包含本次分支的 Git 历史，可在新目录恢复。
- `STATE.json`：精确交付 HEAD、权限边界与验证状态。
- `SHA256SUMS`：除自身以外每一个包内文件的摘要；`PACKAGE_INDEX.json` 提供目录清单。
- `verify_package.py`：解压后只读检查摘要和缺失文件，不安装、不运行项目、不访问网络。
- `guides/`：M6 全页操作 PDF 与最新 R1 十页截图说明。R1涉及页面以新版为准。
- `evidence/`：按阶段保留必要日志、首次失败、截图、PR 快照、类型检查原始诊断。
- `authority-history/`：完整原始计划/交接资料，供追溯；不要直接执行旧的 NEXT_CHAT_PROMPT。

## 最短使用流程
1. 下载、解压到新目录，保留原本项目及本地修改。
2. 执行 `python verify_package.py`，核对全部文件摘要。
3. 把本包根目录 `NEXT_CHAT_PROMPT.md` 的全文交给当前本地项目中的 agent。
4. 本地 agent 先报告实际 Git/工作区/工具链/PR 差异，再执行无阻塞项；模型、付费、发布等必要授权单独询问。

这是源码与工程交接包，不是已经验证过的 Windows 安装程序。不能直接把云端 Linux 编译产物当成 Windows 程序。
