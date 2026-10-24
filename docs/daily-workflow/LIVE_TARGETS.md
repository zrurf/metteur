# 控制与实时输入的固定目标

后续主聊天 Queue/Steer 的持久接收与消费已接入，见 [LIVE_INPUTS.md](LIVE_INPUTS.md)。本页末尾 durable_queue=false 描述固定目标提交时的历史边界；监督及控制/审批持久回执仍待实施。

2026-10-22；源码基线2a536f8加本功能差异。此功能补齐目标绑定，不等同于持久接收/消费回执；DW-03仍在实施。

InterruptRequest新增target_kind、session_id、turn_id、run_id。chat只能发给完整匹配的当前session/turn，blueprint只能发给完整匹配的run；无目标、不一致、错误通道、停止中或没有接收bus均明确拒绝。不会从已结束聊天退到蓝图，也不会随同一session的新turn继续投递。Normal蓝图消息继续走监督入口。原始正文保持空白字符，大小上限1MiB，priority必须是已知值。

AbortChatRequest要求session_id和turn_id；蓝图Pause/Resume/Cancel要求非空且匹配的run_id。运行map读锁覆盖身份核验和发送/标志设置，晚到请求不会改写新的所有者。Stop仍沿用原控制器和broker关闭逻辑，不新建执行器。

capabilities.targeted_controls=true是新Chat控制能力门槛。CLI与Web在旧host拒绝危险降级；Web在任何await之前快照目标，能力查询期间外部修改对象不改投递目标。CLI控制先通过control_target取得具体所有者；该快照只用于控制，idle不是新回合准入许可。能力和控制目标查询不等文件/admission锁，避免Stop先被写文件阻塞。新发送仍使用原admission及宿主最终准入。

| 组合 | 行为 |
|---|---|
| 新Web/CLI → 新host | 携带明确目标；宿主原子核验 |
| 新Web/CLI → 不支持目标能力的host | 控制失败可见，不省略目标重发 |
| 旧无目标请求 → 新host | FailedPrecondition；不猜当前或最新对象 |

TS生成物、网关接口/真实实现、主聊天/监督、CLI及必要测试同完整功能提交。Mock依然只是本窗口演示，不提供持久性证明。主聊天activeTurn来自已验证的Send回执，普通断流语义没有变为detached。

以下是本目标身份提交时的历史边界；Queue/Steer、审批、监督草稿、位置和完整AC此后均已收口，见LIVE_INPUTS.md、APPROVAL_DECISIONS.md、SUPERVISION_INPUTS.md、VIEW_STATE.md和ACCEPTANCE.md。原始提交记录保留如下。

本功能当时没有解决Queue/Steer的durable接收、消费/未消费恢复或审批Unknown对账；当前durable_queue=false。它们当时是下一可执行任务，不得据本功能关闭AC-04/06/15/26/27。主聊天审批当前仍有先清UI再等待RPC的旧行为，必须继续修复并验收；监督草稿、位置和其他DW工作亦未完成。

Windows检查和首次失败保留在daily-workflow-evidence/20261022/targets-*.log：全workspace/all-targets类型检查、协议消费者、真实RPC旧turn拒绝、控制查询不等admission、聊天及审批作用域烟测、本地HTTP/R-batch、L2目标快照/旧能力与聊天/监督回归。具体命令、退出码和结果见REVIEW_HANDOFF。没有生产运行包更新、付费调用、push或合并，不计L3/L4/L5或最终体验验收。
