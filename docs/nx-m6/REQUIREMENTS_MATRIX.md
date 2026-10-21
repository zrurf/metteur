> 当前本地 45/88 完整工程状态已更新到 [LOCAL_ACCEPTANCE_MATRIX.json](../local-handoff/LOCAL_ACCEPTANCE_MATRIX.json) 与 [收尾报告](../local-handoff/COMPLETION_20261021.md)。下表保留云端阶段历史，不代表当前验收。

# NX requirement closure matrix

All 45 task IDs are accounted for. Statuses deliberately distinguish implementation, scoped evidence, provisional UI approval and deferred final acceptance. Historical evidence is not relabelled as a new frozen-RC test.

| Task | Phase | Requirement | Current status | Evidence / implementation | Remaining boundary |
|---|---|---|---|---|---|
| NX-00 | NX-M0 | 冻结真实起点与基线 | DONE_TARGETED (historical) | Original M0 handoff evidence | Not re-counted as M6 testing |
| NX-01 | NX-M0 | 修复 Addon 权限测试的假绿与过紧预算 | DONE_TARGETED (historical) | Original M0 handoff evidence | Not re-counted as M6 testing |
| NX-02 | NX-M0 | 修复缓存请求的断点与排序 | DONE_TARGETED (historical) | Original M0 handoff evidence | Not re-counted as M6 testing |
| NX-03 | NX-M0 | 建立可复现验证门槛 | PARTIAL_PLATFORM | Original baseline and lightweight CI | Native Windows targeted script prepared; user execution pending; full matrix not run |
| NX-04 | NX-M1 | 冻结共同身份、权限与数据契约 | IMPLEMENTED_TARGETED | Original M1 handoff; nx_storage / nx_prompt_snapshots | Final human visual acceptance remains separate |
| NX-05 | NX-M1 | 建立隔离前端预览工作流 | IMPLEMENTED_TARGETED | Original M1 handoff; nx_storage / nx_prompt_snapshots | Final human visual acceptance remains separate |
| NX-06 | NX-M1 | 增加版本化持久记录与迁移 | IMPLEMENTED_TARGETED | Original M1 handoff; nx_storage / nx_prompt_snapshots | Final human visual acceptance remains separate |
| NX-07 | NX-M1 | 项目记忆与证据引用服务 | IMPLEMENTED_TARGETED | Original M1 handoff; nx_storage / nx_prompt_snapshots | Final human visual acceptance remains separate |
| NX-08 | NX-M1 | 记忆查询／写笔记工具 | IMPLEMENTED_TARGETED | Original M1 handoff; nx_storage / nx_prompt_snapshots | Final human visual acceptance remains separate |
| NX-09 | NX-M1 | 批准后接入项目黑板 UI | IMPLEMENTED_TARGETED | Original M1 handoff; nx_storage / nx_prompt_snapshots | Final human visual acceptance remains separate |
| NX-10 | NX-M1 | 双作用域 Prompt 资源服务 | IMPLEMENTED_TARGETED | Original M1 handoff; nx_storage / nx_prompt_snapshots | Final human visual acceptance remains separate |
| NX-11 | NX-M1 | Prompt 快照与 Chat/蓝图上下文接入 | IMPLEMENTED_TARGETED | Original M1 handoff; nx_storage / nx_prompt_snapshots | Final human visual acceptance remains separate |
| NX-12 | NX-M1 | 批准后接入 Prompt 资源 UI | IMPLEMENTED_TARGETED | Original M1 handoff; nx_storage / nx_prompt_snapshots | Final human visual acceptance remains separate |
| NX-13 | NX-M2 | 抽取宿主拥有的回合生命周期 | IMPLEMENTED_TARGETED | M2 NX_M2_REVIEW; continuation modules | Prior Job/child/timer evidence reused, not newly re-counted |
| NX-14 | NX-M2 | 持久化事件、outbox 与 inbox | IMPLEMENTED_TARGETED | M2 NX_M2_REVIEW; continuation modules | Prior Job/child/timer evidence reused, not newly re-counted |
| NX-15 | NX-M2 | 任务状态机、订阅与等待提交 | IMPLEMENTED_TARGETED | M2 NX_M2_REVIEW; continuation modules | Prior Job/child/timer evidence reused, not newly re-counted |
| NX-16 | NX-M2 | 统一准入、dispatch 与副作用租约 | IMPLEMENTED_TARGETED | M2 NX_M2_REVIEW; continuation modules | Prior Job/child/timer evidence reused, not newly re-counted |
| NX-17 | NX-M2 | Job 完成源与 owner 转移 | IMPLEMENTED_TARGETED | M2 NX_M2_REVIEW; continuation modules | Prior Job/child/timer evidence reused, not newly re-counted |
| NX-18 | NX-M2 | 异步子代理句柄与完成事件 | IMPLEMENTED_TARGETED | M2 NX_M2_REVIEW; continuation modules | Prior Job/child/timer evidence reused, not newly re-counted |
| NX-19 | NX-M2 | 定时规则与 fake clock 测试 | IMPLEMENTED_TARGETED | M2 NX_M2_REVIEW; continuation modules | Prior Job/child/timer evidence reused, not newly re-counted |
| NX-20 | NX-M2 | 持久化调度器与启动/关闭集成 | IMPLEMENTED_TARGETED | M2 NX_M2_REVIEW; continuation modules | Prior Job/child/timer evidence reused, not newly re-counted |
| NX-21 | NX-M2 | 续跑授权、根预算和 Stop | IMPLEMENTED_TARGETED | M2 NX_M2_REVIEW; continuation modules | Prior Job/child/timer evidence reused, not newly re-counted |
| NX-22 | NX-M2 | 事件参数与模型上下文适配 | IMPLEMENTED_TARGETED | M2 NX_M2_REVIEW; continuation modules | Prior Job/child/timer evidence reused, not newly re-counted |
| NX-23 | NX-M2 | 三类源恢复与崩溃窗口收口 | IMPLEMENTED_TARGETED | M2 NX_M2_REVIEW; continuation modules | Prior Job/child/timer evidence reused, not newly re-counted |
| NX-24 | NX-M2 | 批准后接入任务等待/唤起 UI | INTEGRATED_PROVISIONAL | PV approval registry; M2 actual browser evidence | User final whole-page sign-off pending |
| NX-25 | NX-M2 | 批准后接入定时管理 UI | INTEGRATED_PROVISIONAL | PV approval registry; M2 actual browser evidence | User final whole-page sign-off pending |
| NX-26 | NX-M2 | 唤起基础完整验收 | LOCAL_CANDIDATE_VERIFIED | M2 live Job/child/timer and host lifecycle | Full all-NX gate not implied |
| NX-27 | NX-M3 | 交接记录与版本依赖清单 | IMPLEMENTED_TARGETED | M3 handoff/launch reports; M6 exact launch smoke | Local controllable provider, not paid vendor evaluation |
| NX-28 | NX-M3 | 蓝图运行提案与具体确认 | IMPLEMENTED_TARGETED | M3 handoff/launch reports; M6 exact launch smoke | Local controllable provider, not paid vendor evaluation |
| NX-29 | NX-M3 | 安全 yield 后启动蓝图 | IMPLEMENTED_TARGETED | M3 handoff/launch reports; M6 exact launch smoke | Local controllable provider, not paid vendor evaluation |
| NX-30 | NX-M3 | 蓝图终态事件与父会话续跑 | IMPLEMENTED_TARGETED | M3 handoff/launch reports; M6 exact launch smoke | Local controllable provider, not paid vendor evaluation |
| NX-31 | NX-M3 | 批准后接入运行确认与交接 UI | INTEGRATED_PROVISIONAL | PV approval registry; M3 actual browser evidence | User final whole-page sign-off pending |
| NX-32 | NX-M3 | 聊天—蓝图—聊天集成验收 | IMPLEMENTED_TARGETED | M3 handoff/launch reports; M6 exact launch smoke | Local controllable provider, not paid vendor evaluation |
| NX-33 | NX-M4 | 选择两类实际模型与冻结任务集 | DEFERRED_NOT_RUN | User 2026-10-15 11:30 deferral | Actual two-model selection/comparison not complete |
| NX-34 | NX-M4 | 请求结构/缓存/用量观测 | GENERAL_LAYER_IMPLEMENTED | docs/nx-m4; request diagnostics and usage | Vendor-specific live cache/cost evidence deferred |
| NX-35 | NX-M4 | 共享底座与模型/角色适配 | PARTIAL_DEFERRED | docs/nx-m4 shared runtime contract | Actual-model-specific adaptation deferred |
| NX-36 | NX-M4 | 必要工具增量与权限目录收口 | IMPLEMENTED_TARGETED | M4 role matrix and bounded tools | No claim of real-model performance improvement |
| NX-37 | NX-M4 | 真实模型匹配对照验收 | DEFERRED_NOT_RUN | User 2026-10-15 11:30 deferral | Actual two-model selection/comparison not complete |
| NX-38 | NX-M5 | Addon 生产取消与审批等待语义 | IMPLEMENTED_TARGETED | M5 real Wasm cancellation/approval tests | Slow HTTP Stop/timeout verified in completion; arbitrary filesystem kernel stalls not forcibly interrupted |
| NX-39 | NX-M5 | 页面 manifest、隔离 Bridge 与生命周期 | IMPLEMENTED_TARGETED | M5 signed page/schema/revocation and CSP checks | Not a full attack or cross-browser matrix |
| NX-40 | NX-M5 | 批准后接入 Addon 页面宿主 UI | INTEGRATED_PROVISIONAL | PV approval registry; M5 actual browser evidence | User final whole-page sign-off pending |
| NX-41 | NX-M5 | 预览检查样例与独立成果验收 | OFFLINE_RENDERER_VERIFIED | M5 source loop + M6 completion real isolated screenshot/console/digest-bound report | Offline rendered evidence implemented; real model opinion remains deferred; arbitrary remote apps excluded |
| NX-42 | NX-M6 | 冻结候选并执行全链路验收 | LOCAL_SCOPE_VERIFIED_GLOBAL_PENDING | Frozen RC1 + five M6 targeted cases | NX37 remains deferred; no claim of complete all-provider acceptance |
| NX-43 | NX-M6 | 规模、平台与迁移回退演练 | PARTIAL_LINUX_ONLY | Migration/rollback; 64-source burst; 1k graph/10k records; 12 DB reopens; kernel lock/crash recovery | Native Windows and wall-clock long soak not run; bounded scale evidence in completion report |
| NX-44 | NX-M6 | 最终交付与状态审计 | LOCAL_DELIVERY_READY | This matrix, user guide, recovery guide, recoverable archive | Remote publication and final human acceptance pending |
