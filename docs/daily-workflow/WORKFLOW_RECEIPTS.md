# 直接控制与蓝图临时输入回执

2026-10-23；基线3119cb0，当前集中候选摘要 dw03-close-batch-source.json。实现状态与首轮检查见 REVIEW_HANDOFF/TASKS；本文定义实际边界，不以新增测试数量关闭完整 AC。

## 现有执行路径

Stop/Pause/Resume 继续使用原 AbortChat/CancelExecution/PauseExecution/ResumeExecution；蓝图临时插入继续使用 SendInterrupt(Urgent) 和既有 InterruptBus。新增 receipt_json={id,revision}，协议、TS生成物、Web、CLI与旧测试消费者一并更新。没有新执行器、自动重试队列或从回执启动工作的后台 worker。

ChatDelivery 的 workflow 查询分支不等待 workspace 文件 admission，提供 capabilities、describe、prepare、read、fence、list。prepare 只持久保存身份与内容，不执行动作；list 分页最多50项，可在重新打开客户端后检查 Prepared/Claimed/Unknown。只读查询和显式 fence 不调用模型、不恢复权限、不执行原动作。

## 控制顺序与未知

- target 固定为 blueprint/run 或 chat/session/turn；owner 由宿主取得。id、操作、正文与目标冲突均拒绝。同一已接受 ID 只返回旧回执，不重设控制标志。
- Pause/Resume 使用 describe 取得的目标 revision；宿主在同一 resource_gate 下验证 revision、持久 Claim/新 revision、设置现有标志，再持久 Accepted。后来控制胜出后，旧请求被拒绝，重复旧 Pause 不能反转 Resume。请求接收不代表执行器已到达暂停/停止边界，运行投影仍是实际状态。
- Stop 优先于普通输入和其他控制，不等待草稿、prepare 或文件 admission。Web/CLI 保留快速目标能力检查，避免旧宿主忽略目标字段；宿主接受固定目标后，即使回执磁盘写失败也设置取消标志。此时只能报告效果未确认，不能说“未停止”。旧目标不会自动转给新 run/turn。
- 丢失 ACK 后只查询/关闭同一 ID。Prepared 可原子变为 Retired，阻止晚到动作；Claimed/Unknown 不重放。没有记录的 Stop 关闭为 Unknown，保守处理可能已经设置取消标志但磁盘失败的情况。跨重启不从回执自动恢复控制或临时输入。
- 老客户端 Pause/Resume 缺少回执身份时明确拒绝，要求升级；旧固定目标 Stop 仍可用但不宣称客户端可对账。旧宿主缺能力时新客户端不静默降级。原 Legacy Emergency 临时中断保留原语义，明确不提供新的持久消费保证；正式临时输入入口使用 Urgent。

## 临时输入消费

Urgent 的原始正文与 Queued 回执在既有 bus 的接收/关闭互斥边界内 sync WAL 后接收。相同 ID/正文不二次入队；不同正文拒绝。正文最多16KiB，不能截断后 ACK。

根 CallLLM 的实际模型边界取得输入前持久 Claim，子代理不会通过 legacy drain 消费宿主拥有的输入。消费记录绑定实际 node ID；只有该节点成功提交 checkpoint 后才变为 Applied。模型/节点失败、提交失败或进程退出时，已 Claim 未提交为 Unknown；run 结束未取得为 Unconsumed。查询会将不再存活的旧 Queued/Claimed 保守归类，不重新执行。

监督临时输入先备份独立原稿和 primary 的 pending_temporary 身份；重开只对账原 ID。接收只清对应编辑版本，较新输入保留；未消费/消费未知保留可检查正文，不能把接收当成已应用。监督普通消息继续使用 ConciergeDelivery 的独立通道。

## 容量、恢复与界面

EXECUTION_STATE 的独立 dw/workflow-receipts namespace，schema1，单记录上限20KiB，总8192回执/16MiB；没有过期淘汰。达到上限、未来 schema、损坏和磁盘失败均明确失败，保留现有记录。sync WAL 复用现有批写，回执不参与文件 rollback/checkpoint 列表；当前候选的 dw namespace 隔离沿用3119cb0。

回执入口在原 Chat 的折叠栏、Execution 原 Requests 区域。默认不查询列表、不增加周期轮询；打开、刷新和检查原回执是显式操作。状态用13px文字，目标/ID/正文放折叠详情；原 IDE 几何保留。切换工作区后旧读取不能填入新列表。

整阶段未决任务/审批分类投影仍由 DW-06 负责；本入口覆盖本契约的直接控制和蓝图临时输入，不声称替代现有聊天投递、监督普通输入、审批决定或恢复副本的独立对账入口。性能、平台、完整生命周期和最终体验按 DW-07～09 合并验收，不在每个协议提交后重复全量验证。
