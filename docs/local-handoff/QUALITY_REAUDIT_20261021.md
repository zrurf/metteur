# 工作区可靠性与完整任务复核 · 2026-10-21

本轮已从原始任务和设计重新检查 NX-00～44 的 45 项任务、SPEC 的 88 个场景，并修复用户报告的日常工作流回归。工程门槛已通过，最终视觉与交互定稿仍待用户确认。旧收尾报告对整体可靠性的完成表述不能证明本轮报告的问题已通过；以本报告及 [逐项证据矩阵](QUALITY_REAUDIT_20261021.json) 为当前入口。

## 候选与审计依据

- 当前分支：`codex/workspace-reliability-20261021`；产品冻结提交：`66c66203cae0c8da48138fc989e2fcc17ad0ebea`；起点：`fb3bb9fa985b5f9a32a7064e994e1801ff4850aa`。
- 原始 TASKS.md SHA256：`6ca2a7dbf2f91f53d2db9a51baae002c4206d8879cb7ece2037b998be6ab0d19`；SPEC.md：`12f45b60171f0c4e710ff05916b0221cdf2466ee59910bd6260843357678b4ff`。完整路径在矩阵中；BASELINE、CODEX_START 和原 R1 PLAN 也以独立摘要保留解释记录。原 R1 的独立导航被用户最新明确要求覆盖；保留原文供追溯。
- 13 个审查域，81 处带摘要的源码引用；每个任务指向具体场景，每个场景绑定具名断言或明确标识的历史/人工证据。状态数量：`{"TECHNICAL_VERIFIED": 85, "HISTORICAL_MATCHED_EVALUATION_RETAINED": 1, "TECHNICAL_VERIFIED_OWNER_VISUAL_PENDING": 2}`。测试总数不代替场景覆盖。
- 621 个运行源码/锁文件摘要：`524bfa97397b31c3fc8fe519624917c0f1eee6dde2c3bb43752088766e3e3a53`。清单：[CANDIDATE_RUNTIME_SOURCE_20261021_R8.json](F:/space/06_Projects/Agent/workspace-review-2026-10-20/CANDIDATE_RUNTIME_SOURCE_20261021_R8.json)。R7 到 R8 仅共享等待提示文案的三处修改；Rust 源码和已测试二进制未改变。
- 原 M1 checkout、旧云端交接包、旧收尾 ZIP、历史付费账本与首轮失败日志均保留。没有修改生产用户工作区。

## 15 项问题的根因与最终实现

| 用户问题 | 最终处理与证据边界 |
|---|---|
| 1、2、4、统一 UI 库 | 移除全部活动栏 Drag to reorder 提示；共用 PageHeader、PageTabs 和 surface 样式，统一标题、宽度、间距、主题和键盘切换。静态产品标题使用 Title Case；用户标题、模型输出与第三方 Addon 内容保持原意。沿用 Vue、既有主题/Tailwind 和 Lucide，无新增 UI 框架或依赖。 |
| 3、6、简化入口 | 六个主入口；Tasks 统一任务与 Schedules，Addon Pages 进入 Settings；Prompt Library 在 Explorer 打开实际来源文本，用文档编辑器和资源 revision 保存。旧链接重定向，Use in Chat 保持不自动发送。 |
| 5、事件驱动 | 一个工作区提交后推送流，前端共用串行合并刷新与重连；宿主 dispatcher 等数据库变更或最近截止时间，移除资源高频轮询。持续时间时钟只负责本地显示。 |
| 7、8、9、ReAct | 在工具边界结束当前 assistant 段，实时与持久化均按发生顺序展示；Context Window 改为紧凑容量条与明细；有效配置加载后选择合法默认模型，删除模型或切换工作区时不保留失效选择。 |
| 10、12、丢失与删除 | 文档由 workspace/path 管理，包含草稿、基线、删除、冲突和保存快照；视图与 Monaco 模型挂接文档。文字和蓝图都保留未保存改动；删除文件的 tab 红色删除线并可 Save to Restore；晚到保存/读取不能覆盖新编辑。 |
| 11、14、文件交互 | 统一树内联名称编辑，焦点、Enter/Escape、错误和重试一致；RPC 成功后才发布列表状态。创建、复制、DSL 导出/编译等统一 create-only，重命名不覆盖目标；跨工作区剪贴板与异步结果有归属。 |
| 13、锁 | 注册表原子打开、关闭阶段拒绝新写入、等待 watcher 退出后释放 OS 锁；窗口切换仅分离本窗口，不关闭其他窗口共用的工作区；并发打开、崩溃与立即重开均有证据。 |
| 15、关联缺陷 | 文件路径与 CAS、Windows junction、Prompt 删除恢复、蓝图派生画布、流退出/重连、Addon lease/请求归属、停止中的子任务及 PWA 更新一并检查修复。真实集成发现等待提示名称遗漏后，实时与恢复状态改为同一来源。 |

## 最终验证

Windows `cargo build/test/clippy --locked --workspace` 完成，908 项通过；Linux/WSL 对应命令完成，921 项通过。两平台均 73 个结果集，零失败、零 ignored。前端依赖锁定安装、应用/Node 类型检查、lint、7 个 live guard、10 个导航断言、预览注册隔离和生产构建通过；完整浏览器 138 项通过，单 worker、零重试。不同平台的条件测试数量差异保留，未合并成一个虚假总数。

原生实测包括 16 并发客户端共享一把工作区锁、12 项文件/推送契约、真实有界 Job、明确拒绝蓝图不执行、批准后仅一个蓝图执行与一次续跑，以及等待后刷新仍回到原会话。完整资源链使用合成 Prompt、确认后的约定、模型意见记录、实际文件/交接读取和黑板记录，另验证修改/删除后的历史不变及越域拒绝。Windows 实际编辑器已检查未保存切换、删除红线和按原内容恢复。

| 验证门槛 | 结果 | 不可变回执 |
|---|---|---|
| native | 通过 | [reliability-native-candidate-full-ftz6060z](D:/BaiduNetdiskDownload/metteur-a7-work/tmp/local-acceptance-20261020/reliability-native-candidate-full-ftz6060z/result.json) |
| linux | 通过 | [reliability-linux-candidate-full-99i76vrq](D:/BaiduNetdiskDownload/metteur-a7-work/tmp/local-acceptance-20261020/reliability-linux-candidate-full-99i76vrq/result.json) |
| browser_full | 通过 | [reliability-final-copy-full-browser-xewhtxk8](D:/BaiduNetdiskDownload/metteur-a7-work/tmp/local-acceptance-20261020/reliability-final-copy-full-browser-xewhtxk8/result.json) |
| headings | 通过 | [reliability-headings-final-il_eoxlh](D:/BaiduNetdiskDownload/metteur-a7-work/tmp/local-acceptance-20261020/reliability-headings-final-il_eoxlh/result.json) |
| addon_ownership | 通过 | [reliability-addon-ownership-third-6w58uu42](D:/BaiduNetdiskDownload/metteur-a7-work/tmp/local-acceptance-20261020/reliability-addon-ownership-third-6w58uu42/result.json) |
| web_checks | 通过 | [reliability-web-final-copy-checks-wfgny66b](D:/BaiduNetdiskDownload/metteur-a7-work/tmp/local-acceptance-20261020/reliability-web-final-copy-checks-wfgny66b/result.json) |
| production_build | 通过 | [reliability-production-web-r8-kb8s1w89](D:/BaiduNetdiskDownload/metteur-a7-work/tmp/local-acceptance-20261020/reliability-production-web-r8-kb8s1w89/result.json) |
| full_resource_chain | 通过 | [reliability-current-full-resource-chain-retry-9fn3t0ov](D:/BaiduNetdiskDownload/metteur-a7-work/tmp/local-acceptance-20261020/reliability-current-full-resource-chain-retry-9fn3t0ov/result.json) |
| native_contracts | 通过 | [reliability-r7-native-contracts-b9rc8arg](D:/BaiduNetdiskDownload/metteur-a7-work/tmp/local-acceptance-20261020/reliability-r7-native-contracts-b9rc8arg/result.json) |
| native_job_and_decline | 通过 | [reliability-r7-native-job-current-labels-c2u16nwu](D:/BaiduNetdiskDownload/metteur-a7-work/tmp/local-acceptance-20261020/reliability-r7-native-job-current-labels-c2u16nwu/result.json) |
| native_blueprint_reload | 通过 | [reliability-r8-native-blueprint-reload-_w1qftpp](D:/BaiduNetdiskDownload/metteur-a7-work/tmp/local-acceptance-20261020/reliability-r8-native-blueprint-reload-_w1qftpp/result.json) |

前端全量测试的 fixture 改为真实文件树/存储行为，未用总是返回空文件的 mock 掩盖保存问题。外部旧标题定位器失败、错误 fixture 工具名和等待提示遗漏的首次日志分别保留；最终回执在矩阵中，不删除失败记录制造全绿。

## 每一步的审查范围

- **foundation**：核对实际分支、锁文件、原生/WSL工具链与日志；资源迁移先建一致备份再写 schema，CAS 与重放回执同批持久化，未知 schema 拒绝写。原始云端完成标记不作为本轮证据。跨平台缺失用例、历史模型结果与人工视觉确认分别记录。
- **memory**：按规范化项目身份绑定记录；模型笔记不能写入执行事实/用户身份/确认字段；正文编辑清除确认，提升绑定精确 revision/digest；文件证据有大小和目录限制，查询有分页/字节预算，迟到 UI 请求不能重定向草稿。
- **prompts**：项目/个人库按 scoped ID 选择；文件与索引用 durable intent 对账，显式选择正文冻结为 user-level snapshot；历史/run manifest 不追随新源。新增 Windows junction 拒绝和精确 tombstone 修订恢复；正文在统一编辑器保存，元数据并发变更需先审阅当前修订。
- **turn**：宿主在完整工具批次结果持久化后提交 WAITING 并释放前台槽位；host event 与用户授权不同源，恢复不从 UI 重建上下文。修正 assistant/tool 段边界，直播/恢复均按调用顺序；整轮耗时保留一次，context 的未知值与零值分开显示。
- **events**：source/event/dispatch 身份和代次由宿主生成；wait/context 和 dispatch ack 原子提交；已启动不明进入 InDoubt，不能因租约过期重放副作用。Stop/预算/新用户修订在准入复核。数据库提交发合并失效通知，页面手动/推送读取共用队列；宿主等待通知或最近截止时间，移除资源高频轮询。
- **jobs**：仅已登记来源转移为 TaskOwned；普通聊天断开仍清理。取消按 task/workspace owner 隔离；重启的旧 PID 不能证明结果，写副作用未知需人工处理。watch lag/客户端关闭显式结束订阅，不持有过期工作区锁。
- **child**：后台子代理的 read-only allowlist 由宿主固定，清除个人库、父上下文和 Addon 片段，费用计入根账本。子代理取消改为 DB 通知/授权截止时间，Stop 取消进行中模型 future，未知用量保留 reservation，不能退款或当零。同步工具保持兼容。
- **timer**：时区/DST/回拨由确定性时间参数测试；occurrence 绑定规则和任务代次；离线按 coalesce/skip，关闭不自动重新打开目录。编辑采用精确 revision/digest 确认。Schedules 作为 Tasks 页签，保存和推送共用刷新队列。
- **launch**：确认绑定文件、Prompt、函数、Addon manifest 的内容身份；源消息只读固定终点且默认不开放完整会话；安全 yield 后领取蓝图槽位，intent 重放查询同一 run。终态结算后恢复原会话；失败/Stop/report-only 不自动重复执行。画布改用 workspace 文档草稿、CAS 和独立 canvas ID。
- **cache**：核对最终序列化请求，tools/system/messages 共用 marker 预算并排序，日期不污染稳定前缀；本地 HTTP 验证流/非流和工具第二轮。缓存/用量未报告保持 coverage unknown，历史账本不追溯改写。
- **models**：角色工具边界在宿主目录收口，不靠 Prompt 扩大权限。保留两类 DeepSeek 配对/留出和所有首次失败、用量回执；本轮未新增付费调用，不把旧模型测量称为当前二进制的新性能评测。自动模型选择由有效配置/default/fallback 驱动，删除配置后不保留失效模型。
- **addon**：权限拒绝断言具体原因；人工等待与 CPU/I/O timeout 分离，核对文件/journal 保留已发生效果。包按已验证不可变字节执行，Windows junction 也拒绝。Bridge 的身份/nonce/lease/方法/schema/能力逐次核对，iframe 无主站同源权限。检查目标和 human acceptance 绑定 preview revision，acceptance 不授予运行权限；入口移至 Settings。
- **ui**：复用原 Vue/Tailwind/theme/Lucide，不新增 UI 框架或依赖。六个主入口，公用页面留白/标题/页签/按钮，静态标题 Title Case；Prompt 文件进入通用编辑器。文档跨 tab/工作区保留，关闭明确确认丢弃，删除可恢复；Explorer 行内输入具备焦点/键盘/失败状态。PWA 监听已在安装和未来更新，保留用户控制的刷新。

| 任务 | 原始任务标题 | 对应设计场景 |
|---|---|---|
| NX-00 | 冻结真实起点与基线 | E2E-05 |
| NX-01 | 修复 Addon 权限测试的假绿与过紧预算 | AD-01, AD-03 |
| NX-02 | 修复缓存请求的断点与排序 | HC-01, HC-02, HC-04 |
| NX-03 | 建立可复现验证门槛 | E2E-05 |
| NX-04 | 冻结共同身份、权限与数据契约 | EV-09, PF-01, BB-04 |
| NX-05 | 建立隔离前端预览工作流 | UI-01, UI-02, UI-03 |
| NX-06 | 增加版本化持久记录与迁移 | BB-07, E2E-06 |
| NX-07 | 项目记忆与证据引用服务 | BB-01, BB-02, BB-03, BB-04, BB-05, BB-06, BB-07 |
| NX-08 | 记忆查询／写笔记工具 | BB-02, BB-04, EV-09 |
| NX-09 | 批准后接入项目黑板 UI | UI-02, UI-03, UI-04, UI-05, BB-01 |
| NX-10 | 双作用域 Prompt 资源服务 | PF-01, PF-02, PF-08, PF-09, PF-10 |
| NX-11 | Prompt 快照与 Chat/蓝图上下文接入 | PF-03, PF-04, PF-05, PF-06, PF-07, PF-10 |
| NX-12 | 批准后接入 Prompt 资源 UI | UI-03, UI-04, UI-06, PF-01, PF-03, PF-06 |
| NX-13 | 抽取宿主拥有的回合生命周期 | EV-01, EV-15, EV-16, LF-02 |
| NX-14 | 持久化事件、outbox 与 inbox | EV-02, EV-03, EV-09, EV-10, EV-14 |
| NX-15 | 任务状态机、订阅与等待提交 | EV-01, EV-02, EV-04, EV-05, EV-15 |
| NX-16 | 统一准入、dispatch 与副作用租约 | EV-06, EV-13, LF-04, LF-08 |
| NX-17 | Job 完成源与 owner 转移 | LF-01, LF-03, LF-04, LF-07, EV-02 |
| NX-18 | 异步子代理句柄与完成事件 | LF-05, LF-06, LF-07, EV-11 |
| NX-19 | 定时规则与 fake clock 测试 | TM-01, TM-02, TM-03, TM-04, TM-08 |
| NX-20 | 持久化调度器与启动/关闭集成 | TM-04, TM-05, TM-06, TM-08 |
| NX-21 | 续跑授权、根预算和 Stop | EV-07, EV-08, EV-11, EV-12, TM-07 |
| NX-22 | 事件参数与模型上下文适配 | EV-09, EV-10, EV-16, HC-02, HC-07 |
| NX-23 | 三类源恢复与崩溃窗口收口 | EV-02, EV-03, EV-13, EV-14, EV-15, LF-03, LF-08, PFM-03 |
| NX-24 | 批准后接入任务等待/唤起 UI | UI-02, UI-03, UI-04, UI-05, UI-07 |
| NX-25 | 批准后接入定时管理 UI | TM-02, TM-05, TM-07, UI-03, UI-07 |
| NX-26 | 唤起基础完整验收 | E2E-02, E2E-03, EV-01, EV-06 |
| NX-27 | 交接记录与版本依赖清单 | BP-01, BP-05, PF-07 |
| NX-28 | 蓝图运行提案与具体确认 | BP-01, BP-02, BP-04, EV-09 |
| NX-29 | 安全 yield 后启动蓝图 | BP-03, BP-04, BP-08, EV-15 |
| NX-30 | 蓝图终态事件与父会话续跑 | BP-06, BP-07, BP-08, EV-07, EV-14 |
| NX-31 | 批准后接入运行确认与交接 UI | UI-03, UI-04, UI-05, UI-07, BP-01 |
| NX-32 | 聊天—蓝图—聊天集成验收 | E2E-01, E2E-03, E2E-04, BP-08 |
| NX-33 | 选择两类实际模型与冻结任务集 | HC-06, HC-07 |
| NX-34 | 请求结构/缓存/用量观测 | HC-01, HC-02, HC-05, HC-07 |
| NX-35 | 共享底座与模型/角色适配 | HC-03, HC-04, HC-06 |
| NX-36 | 必要工具增量与权限目录收口 | HC-03, EV-09, LF-05, BB-04 |
| NX-37 | 真实模型匹配对照验收 | HC-06, HC-05, E2E-01 |
| NX-38 | Addon 生产取消与审批等待语义 | AD-02, AD-03, LF-08 |
| NX-39 | 页面 manifest、隔离 Bridge 与生命周期 | AD-04, AD-05, AD-06 |
| NX-40 | 批准后接入 Addon 页面宿主 UI | UI-03, UI-04, AD-04, AD-06 |
| NX-41 | 预览检查样例与独立成果验收 | AD-07, AD-08, UI-03, UI-07 |
| NX-42 | 冻结候选并执行全链路验收 | E2E-01, E2E-02, E2E-03, E2E-04, E2E-05 |
| NX-43 | 规模、平台与迁移回退演练 | PFM-01, PFM-02, PFM-03, E2E-05, E2E-06 |
| NX-44 | 最终交付与状态审计 | E2E-06, UI-01, UI-03 |

每行均已完成代码复核与证据绑定。涉及人工视觉和真实模型的场景，状态在矩阵中单独保留，不能据此表格推断为本轮全部付费重测或用户已签字。

## 当前页面与保留事项

最终本地实例：[打开 Tasks](http://127.0.0.1:1801/work/w4oASYddL-qk/tasks)，使用原生生产程序、最终 R8 前端和隔离合成数据。本地 fixture 模型不产生付费调用。Addon Pages 可从 Settings 打开签名示例；Prompt Library 位于 Explorer。当前运行信息见外置 ACTIVE_OWNER_REVIEW.json。

- 最终视觉仍为 `PENDING_OWNER`。已有 PV 批准、自动截图或模型意见不批准这次新设计。UI-01/03 保留人工决定，其他既有视觉签字限制也未消除。
- Clippy 仍有既有 27 条库诊断和一条测试模块诊断；没有新增抑制规则。生产前端仍有 Monaco/语言包大 chunk 提示；没有宣称零警告或完成多日 soak。
- 本轮新增付费调用为 0。原有 379 次生成、估算 US$1.82139858 和 US$5 合成数据上限仅为历史记录；未将 fixture 结果冒充新的 DeepSeek 模型质量比较或实际账单。
- 旧 ZIP `Metteur_本地收尾_fb3bb9f_20261021.zip` 是旧版本。当前产品代码以冻结提交和新的 Git 恢复包为准；不能从旧包启动并宣称已经包含这轮修复。
- 发布继续遵循 fork 顺序快进、保留独立提交、不强推、不推 origin、不自动合并。PR #8 已被上游合并，后续新建 Draft PR，发布实况写外置回执。
