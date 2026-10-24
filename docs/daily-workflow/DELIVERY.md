# 手工发送的持久接收协议

最新主聊天 Queue/Steer 持久接收、正文和消费契约见 [LIVE_INPUTS.md](LIVE_INPUTS.md)。下方“Queue/Steer 待接入”“durable_queue=false”是手工 Send 提交阶段的历史边界，以新文档为准；蓝图监督、控制和审批已在SUPERVISION_INPUTS.md、WORKFLOW_RECEIPTS.md、APPROVAL_DECISIONS.md完成；全阶段结果见ACCEPTANCE.md。

固定实时输入/控制目标增量见LIVE_TARGETS.md；该增量不提供Queue/Steer的durable消费回执。

2026-10-22 Composer 接入增量（源码基线 f60b614）：正式主聊天使用持久输入控制器与 DeliveryAttempt。发送前分别 ACK 固定原稿和当前编辑稿的 pending ID；Host Received 和执行结果分开，拒绝/未知/晚到回执不覆盖新输入。重开选取副本后只读/retire 原 ID，不自动发送；同窗口跨会话回调按 workspace/epoch 隔离。共享 Composer 覆盖 composition/229、候选确认后的 Enter、长按、键盘/菜单/按钮的共同守卫。监督调用仅同步修复 ACK 清理和失败覆盖，独立恢复与其协议后续完成。Queue/Steer 仍是原 Interrupt 协议，不能据此声称有持久固定目标回执；控制/审批亦待后续。

Read/Retire 返回当前 source_available（当前 GetChatSession ACL 与直接存储读取），不恢复来源、不进行 load_thread 迁移。已接收但来源缺失的恢复输入保持原文与 ID，允许手工复制；回执存在不等于当前可读或重新获权。Windows RPC7、客户端27、L2聊天9及受影响回归18通过；首轮失败、命令和边界见 REVIEW_HANDOFF.md。无 L3/L4/L5 替代、模型调用0。

2026-10-22 增量：客户端还要求 capabilities.pending_reconciliation=true，准备新 Send 时先调用 reconcile_pending。宿主只扫描当前认证 owner/project scope 中已备份的 chat/new_chat pending_submission 身份；未接收的原 ID 持久关闭，已接收的返回原 turn/phase。监督身份不混入手工 Send ledger，损坏记录隔离，原始副本不改写。失败时阻止新 Send，不静默降级或执行旧输入。这覆盖浏览器重开后旧请求仍可能晚到的窗口；自动恢复/发送仍禁止。

该增量在 9aa23cc 上独立实施：真实 RPC6项、客户端9项、SSE3项及 app 类型/daemon clippy 通过（仅既有27项lib警告）。证据为 pending-delivery-rpc-first、pending-delivery-client-first、pending-delivery-transport、pending-delivery-clippy-first.log。没有新生产运行包或真实模型调用；主聊天/监督草稿与 Composer 仍待接入。

DW-03 第一个独立功能，2026-10-22。包括 daemon 接收/查询、Web SSE 适配及独立客户端状态机。Composer、未决输入恢复、Queue/Steer、控制与 IME 仍待后续接入；定向通过不代表整个工作包或 AC 已关闭。

先查询增量 ChatDelivery RPC 的 capabilities。新请求在既有 SendChat.options_json 提供 `delivery: { id, expected_revision, new_session? }`，身份为非 nil UUID。new_session 在显式 Send 接受后才成为新会话。旧请求保持原语义和统一准入；旧服务返回 Unimplemented，新客户端不声称旧服务有新保证；mock 不提供伪耐久回执。durable_queue=false 如实表达其边界。

接收前核验准入、session/revision、Prompt 选择和模型配置，捕获原 paired checkpoint。原始用户输入/context、transcript、单调 session revision、回执及关联新 continuation task 的 expected revision 一次 sync WAL 提交。再将 Started 持久化后进入原 ReAct runner。不新增执行器或普通聊天 detached 语义。拒绝不增加用户消息或 session revision。

回执保存输入指纹、session/turn/checkpoint、accepted revision、phase 和有界 outcome，不重复保存正文。指纹含原 session/message/history/options 精确内容。相同 ID/内容返回原回执；异内容拒绝。重复只产生 delivery/delivery_replay，不第二次调用模型/工具、不伪造新用量。Accepted 是输入接收；之后模型失败仍是已接收输入的执行结果。

客户端未知只按原 ID 对账，不自动重传或新建 ID。恢复未决身份只对账，不恢复 full/AllowOnce 权限。**一次 read 不存在不是明确未接收**，原请求仍可能晚到。retire 与 accept 使用相同 admission/resource gate：未接受则写永久 Retired 凭据、拒绝晚到发送；接受已胜出则返回原回执，不取消或替换执行。只有 Retired 才能归为未接收、允许用户新提交；retire 结果未知时仍保留原 ID。

接收后启动前崩溃留下 Accepted；Started 的效果保守视为未知。本实现不自动重放任一种窗口。普通断流继续取消，只有已授权 detached policy 延续。文件/context rewind 不回退 ledger。整库灾备回滚可能回退凭据，需保留最新数据库/回执；不能据缺失进程推断从未产生效果。

存储为 EXECUTION_STATE 独立 namespace `dw/delivery/<SHA256(canonical workspace, authenticated subject)>/`，schema1。单回执4KiB，每 owner/workspace 8192条/16MiB，分页50；永久轻量凭据不静默淘汰，满额拒绝新输入。原 message+history+options 上限1MiB。未知 schema/损坏回执 fail closed，不当作不存在。已删除会话 revision 不归零，新聊天不复用已存在/已删除身份。owner 来自认证连接。

Windows 存储8、真实 RPC/原 mock runner5、客户端状态机7、SSE与原序列3项定向通过。首轮 RPC 合成 continuation 样本预算过小被真实守卫拒绝；修正样本预算及 max_tokens 后通过，没有放松守卫。命令、源码候选、首次失败与未测项见 REVIEW_HANDOFF.md 和工作区 daily-workflow-evidence/20261022。生产完整故事、Linux、真实模型、用户使用及剩余入口未验收。
