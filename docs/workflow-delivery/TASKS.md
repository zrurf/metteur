# 编程工作流交付任务表

更新：2026-10-24。用户批准整个阶段并要求直接实施。范围与契约见 [PLAN.md](PLAN.md)。分支 `codex/workflow-delivery-20261024`，起点 `589f307`，单代理串行。DW 原验收和原件不改写。

## 当前状态

WD-00～07 已完成，40 项验收按下表证据及明确的继承边界收口。四类真实模型任务均有正常结束、独立验收和实际监督调用的完整交付；首轮 12 单元与后续 8 单元分开报告，累计 179 请求，费用保守上界 US$0.459912，未决计费 0。首轮文件通过但迭代耗尽不算完整成功，详见 MODEL_REPORT。正式 19103 已更新至产品提交 f2057b5，原三个工作区冷备与升级前后、界面检查后的数据对照一致。交接与推送回执入口见 REVIEW_HANDOFF.md。只复验失败和受影响范围，未重跑原 DW 全矩阵。

| ID | 完整功能包 | 依赖 | 状态 | 证据/退出条件 |
|---|---|---|---|---|
| WD-00 | 基线、设计遗漏核对、完整阶段契约 | 无 | VERIFIED（文档） | 起点/工作树/设计映射已读；静态复核，不冒称运行验收 |
| WD-01 | 四类目录、编译、契约、实例与预检 | WD-00 | VERIFIED | 共享 7 项；只读 RPC 预检；普通画布往返、具体版本确认与真实运行 |
| WD-02 | 两组隔离交互预览及确认 | WD-00/01 | VERIFIED（预览范围） | 用户确认 r2；证据和边界见 PREVIEWS.md |
| WD-03 | 宿主证据投影、时效与持久人工审阅 | WD-01 | VERIFIED | Windows 5 组、Linux 6 组；回执持久化/上限/ACL；真实服务断连与刷新对账 |
| WD-04 | 正式创建/确认/结果/继续流程接入 | WD-02/03 | VERIFIED | 原聊天草稿保留、真实创建/保存/确认运行/逐项结果/继续草稿，未决审阅身份恢复 |
| WD-05 | 固定四任务、三模式运行器及独立评判 | WD-01/03 | VERIFIED | oracle/未修改基线/越界/mutant；并发计费硬限额；宿主生成文件精确例外 |
| WD-06 | 获批真实模型 12 单元与修复 | WD-04/05、具体新预算 | VERIFIED | 首轮 12 单元全保留；4 个中间补验和 4 个修复后监督样本；US$3/240 总请求/12 每单元，同账本审计扩展 |
| WD-07 | 兼容、使用/恢复、候选与完整收口 | WD-01～06 | VERIFIED | 131 文件冷备、正式升级、原数据最终对照、使用/交接文档完成；推送回执见 REVIEW_HANDOFF |

状态只按事实推进：PLANNED → IN_PROGRESS → IMPLEMENTED → VERIFIED。依赖具体确认时标 WAITING_PREVIEW/WAITING_INPUT，不停止其他可独立推进工作。每个完整可构建功能按原 D17 单独本地提交，不制造碎片。远端遵守既有 fork/快进边界，不推 origin、不强推或合并。

## 验收矩阵

证据根 `F:/space/06_Projects/Agent/workflow-delivery-evidence/20261024`。下表按实际检查归档；继承的旧机制另注明，不计作本阶段重跑。预览只证明交互，真实服务和真实模型另列。

| AC | 场景及通过标准 | 首轮/证据 |
|---|---|---|
| 01 | 四类目录完整；固定英文文案、来源 revision、适用/不适用、所需输入可读 | VERIFIED：templates-catalog-retest.log；live-ui-first.json；目录与 USAGE 输入说明 |
| 02 | 四类用真实目录编译并通过静态校验；无虚构节点/隐藏服务 | VERIFIED：共享四类普通图测试；真实模型四类实际编译/执行 |
| 03 | 实例独立 UUID；同请求重试一致；同名实例不相互覆盖 | VERIFIED：retry_is_stable_and_same_named_instances_are_isolated；live-create-first.json 使用 create-only |
| 04 | 严格字段/大小/数量/路径/要求 ID/命令校验；预检零执行/零模型 | VERIFIED：共享严格输入反例；preflight-config-host-test.log 零命令/模型与文件预检 |
| 05 | 修 bug 预期基线失败、最终通过；非预期退出/超时不能算重现成功 | VERIFIED：wrong_baseline_blocks_implementation_and_nonzero_final_remains_failed；修 bug 真实基线失败及最终成功；状态必须 exited，超时不计通过 |
| 06 | 补测试捕获冻结缺陷、重构保持行为/接口、小功能三类场景及回归 | VERIFIED：evaluation-grader-first.log；MODEL_REPORT 四类定向交付/独立断言/mutant |
| 07 | 元数据随保存/加载/恢复/普通编辑保留；模板更新不改实例；高级编辑不被表单覆盖 | VERIFIED：live-confirmation-tool-fix.json 保存及确认部分、live-confirmed-result.json；原生契约保留/DSL 拒绝有损导出 |
| 08 | 契约/命令/资源变化导致旧确认失效；普通蓝图原有警告规则保持 | VERIFIED：共享篡改反例；preflight-config-host-test.log 配置变更拒旧确认；两个 UI 入口取消零执行 |
| 09 | 模板→填写→阶段/节点→取消/修改/冲突的完整隔离预览 | VERIFIED：preview-r2-first + preview-r2-continuity-retest；PREVIEWS |
| 10 | 结果各状态→证据→接受/请求修改→新提案的完整隔离预览 | VERIFIED：PREVIEWS r2 状态/冲突/后续流程；实际服务证据另见 26 |
| 11 | 浅/深色、窄屏、键盘/焦点/IME、长内容；主要操作不被遮挡 | VERIFIED（技术范围）：r2 窄屏/浅深色/键盘/组合事件、截图检查；物理 IME 沿用旧人工接受，不代签本阶段实测 |
| 12 | 用户明确确认 WD-PV-01/02 revision；fixture 不冒充生产 | VERIFIED：用户“确认 r2，按这版接入”；PREVIEWS.md |
| 13 | 要求投影绑定宿主持久 run/蓝图版本；不读取最新图替代旧图 | VERIFIED：real_process_results_are_version_bound_and_file_changes_invalidate_only_current_acceptance；实际 run 审计冻结根图 |
| 14 | 检查由真实 current invocation/check 产生；伪造/旧 attempt/无记录不能通过 | VERIFIED：同宿主测试的命令输出缺失、current=false、版本缺失反例 |
| 15 | passed/failed/missing/skipped/stale/needs-review 各有独立证据和原因 | VERIFIED：宿主真实 passed/failed/missing/stale/needs_review 与 skipped 分支逻辑；预览覆盖状态呈现，不把模型总结当证据 |
| 16 | 文件在检查后/结束后改变、删除、不可读均失效；历史结果不被改写 | VERIFIED：host-tests-first.log 文件改写/删除；live-run-first.json；历史 checkpoint 字节不变 |
| 17 | 证据路径越界/链接逃逸/元数据拒绝；大小/覆盖范围明确 | VERIFIED：宿主大小/元数据/缺失反例；linux-workflow-first.log 实际符号链接拒绝 |
| 18 | 用户接受/请求修改绑定具体摘要、主体、run、requirement，不授予执行权限 | VERIFIED：真实 RecordWorkflowReview、主体/摘要绑定；live-review-ui-first.json 成功写入部分 |
| 19 | 同请求幂等、同 ID 异内容拒绝、CAS 多窗口冲突、迟到请求保留正确归属 | VERIFIED：review_receipts_survive_reopen_with_conflicts_stale_files_and_subject_isolation；回执精确字段客户端守卫 |
| 20 | 断连未知先对账；刷新/宿主重启记录不丢、不重复记录或运行 | VERIFIED：live-recovery-first.json 已提交丢 ACK 不重写、未提交刷新恢复原请求；宿主关闭重开记录完整 |
| 21 | 模型/插件无人工验收入口；跨工作区/run/主体读写不能绕过 ACL | VERIFIED：rpc_enforces_workspace_acl_user_fields_redaction_and_query_has_no_write_effect；注册入口隔离 |
| 22 | 有界修订/正文/总量；额度耗尽显式失败且历史保留 | VERIFIED：64 修订上限/2049 字节拒绝/历史回执保留；恢复 revision64 客户端守卫；资源存储 128/16KiB 约束 |
| 23 | 旧图无要求/未知 schema/损坏证据诚实降级，无虚假验收通过 | VERIFIED：legacy_graphs_keep_their_existing_validation_behavior；未知 schema 拒绝；缺坏证据不推断通过 |
| 24 | 创建走 create-only/版本/mirror，失败保留表单草稿，不产生幽灵图 | VERIFIED：live-create-first.json 创建/ACK/镜像；既有 create-only 原子守卫复用；预览失败留稿 |
| 25 | 聊天启动冻结原会话/版本/完成策略；独立运行不伪造来源 | VERIFIED：live-source-confirmation-retest.json 原会话保留部分；模板提案复用 DW 精确交接；独立运行来源不造会话 |
| 26 | 阶段/要求/证据/人工审阅真实端到端，回执失败不能显示接受 | VERIFIED：live-run-first、live-review-ui-first 成功写入、live-review-reopen-retest、live-recovery-first、live-confirmed-result |
| 27 | 失败结果带回草稿需显式发送；保留任务账本/预算，不隐式重跑 | VERIFIED：live-followup-retest.json 真实结果进入草稿，零 Send/Run；原任务预算机制未改 |
| 28 | Stop/预算耗尽/审批/占用/未知效果继续原守卫，主聊天与监督不串线 | VERIFIED（继承+定向）：DW foreground/receipts 既有守卫未改；本阶段迭代/请求上限与失败保留、独立监督真实审计 |
| 29 | 草稿 ACK/旧结果/原数据兼容；新增表单关闭/刷新可恢复 | VERIFIED：live-plan-recovery-reopen.json 未完整表单刷新恢复、partial-form shape guard、真实审阅刷新恢复；旧正文/图与位置守卫复用 |
| 30 | 四个完整任务及独立评判冻结，开发样本和评测分离 | VERIFIED：model-evaluation-first/frozen.json 首次请求前冻结；外部独立判据，开发 oracle 不进入工作区 |
| 31 | 三模式条件、启动初态、顺序、监督差异明确，运行器拒绝未授权费用 | VERIFIED：MODEL_REPORT 原始顺序/条件/监督未到达事实；后续四个监督实际调用，保留独立批次 |
| 32 | 离线验证真实文件/命令评判，能抓故意缺陷/意外改动；模型总结不参与判定 | VERIFIED：evaluation-grader-first.log + evaluation-boundary-retest.log，全部真实 Python 子进程判据 |
| 33 | 费用账本/请求硬上限/未知用量/中断恢复/防重复请求检查通过 | VERIFIED：per-cell-budget-first.log 并发最后一槽；原账本保留的 144 上限拒绝与显式 240 扩展、未决用量禁止继续 |
| 34 | 获批 12 单元各有完成/失败/过限记录，全部失败与重试原因保留 | VERIFIED：MODEL_RESULTS.json 全 12 首轮+8 定向样本；错误流勘误及所有失败保留 |
| 35 | 四类各有真实完整交付通过；产品缺陷修复，模型差异按实际报告 | VERIFIED：model-job-fix-supervised 四类完整成功，实际 supervisor 各 1；重复作业通知产品缺陷修复/定向回归 |
| 36 | 成本含规划/执行/重试/监督；时间/人工/缓存/unknown 分开，不预设提升 | VERIFIED：MODEL_REPORT 179 请求全口径，时间/准备成本未测/缓存覆盖/未知与费用上界明确 |
| 37 | 受影响 Rust/Web 类型、lint/build/定向回归与生产链通过，无新增告警 | VERIFIED：Windows 宿主/共享与新回归；Web 类型/生产构建/窄 lint；wire-roundtrip-regression；Clippy 仅 28 既有告警 |
| 38 | Windows/Linux 受影响宿主范围、旧数据库/图文件与冷备恢复有证据 | VERIFIED：Windows 宿主5/Linux宿主6及 Linux 作业回归通过；formal/cold-backup.json 131 文件哈希一致；formal-final-preservation.json baselineMatched=true。旧冷恢复格式未改，复用 DW RESTORE_GUIDE 证据，不冒称本阶段再次恢复 |
| 39 | 使用、限制、失败继续与恢复说明；候选源码/资产/原数据对应 | VERIFIED：USAGE.md、REVIEW_HANDOFF.md；formal/artifact-manifest.json 522 源码/387 Web 文件/2 程序与产品 f2057b5 对应；formal-surface.json 真实正式入口 |
| 40 | 全任务/AC/预览/模型结果一致，无用旧证据冒充新验收；阶段交付清单明确 | VERIFIED：本表、PLAN/PREVIEWS/MODEL_REPORT/USAGE/REVIEW_HANDOFF 一致；失败原件及继承边界保留；正式交付与远端快进回执入口见交接 |

## 实施注意

每个场景集中完成正反例后一次执行，保留首轮日志；只复测失败及受影响范围。文档变更只验链接/差异。未受影响历史证据明确引用。旧 IME/S3 与观察时长的接受决定不重新排期；新增输入仍做适用键盘/组合事件技术检查，实际物理操作不代签。

## 设计复核补齐记录

已补实例与来源身份、模板升级/高级编辑、导出兼容、预检零副作用、基线失败判定、要求与证据绑定、文件后变更、人工决定幂等/权限/上限、无契约旧图、未知状态、预算和四类完整交付、对照冻结与失败保留。旧 `docs/local-handoff/REMAINING_TASKS.md` 顶部过期状态同步指向本阶段与 DW 收口。原 SPEC/TASKS 历史文件保留，不改历史授权。
