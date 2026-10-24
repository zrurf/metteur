# 监督草稿和正常消息接收

2026-10-22；源码基线 e4f2a69 加本功能完整差异，提交前摘要 supervision-input-source-before-commit.json。只覆盖独立监督入口的草稿和正常模型消息；既有临时 Urgent 插入保存独立原稿，但其 durable 消费回执仍待后续。控制/审批、位置恢复与完整 AC 继续实施。

## 宿主契约

新增 `ConciergeDelivery(ResourceRequest) -> ResourceResponse`，操作为 capabilities、reconcile_pending、read、read_or_retire；查询不会启动模型、创建请求、审批、修改运行或等待文件写锁。当前 GetConciergeState 权限和认证 owner 仍需核验，run/conversation/message ID 均固定。新客户端要求 durable_intake/read_or_retire/pending_reconciliation 能力，旧服务不足时保留草稿，不降级成不受保护发送。

既有 SendConciergeMessage 的 message_id 继续作为逻辑输入身份。正常接收在原 oversight gate 下将身份和原始正文/history 一次 sync WAL 提交；processing 首事件现在携带真实已持久化的 Turn，Gateway 保留它。Accepted 与模型成功、请求转交、监督处理和批准权限分开。重复同 ID/内容只返回原记录，不调用第二次 provider；异内容、owner 或目标冲突拒绝。

read_or_retire 在同一 gate 内关闭未接收身份，晚到输入不能复活。已有身份但历史缺失是 Unknown；旧两次写入崩溃留下的半记录不能重新执行。历史 tuple 身份仅按原 local 主体兼容，新身份为带 schema/owner/run/conversation/digest 的记录；未来 schema 和损坏内容保守失败。新格式或 retire 记录在旧宿主也不会匹配旧 tuple 请求，避免旧客户端将原 ID 当新输入。

每工作区监督身份最多 8192 个，总身份与历史正文最多 16 MiB；单 run 最多 128 条监督消息，正常输入 16 KiB，单历史读取 4 MiB，旧身份读取 20 KiB。达到上限保留现有记录并明确拒绝；finish/recovery 更新也受容量约束，不静默淘汰未决原文。

## 草稿与界面

监督草稿复用 project/owner/window 的 RecoveryCopy，entity 为准确 run UUID，与主聊天和其他运行分离。发送前确认固定原稿及当前稿的 pending 身份，收据只清匹配版本，新编辑保留。重开由恢复列表明确选取，恢复输入不调用模型、不赋予旧权限或选择模型；原 ID 只对账/关闭。来源不可用时保留原文和身份，不创建源运行。未知、拒绝、未备份副本均可查看/复制或导出。

保留原 IDE 几何，默认只增加 13px 的备份/输入状态与必要操作，长诊断在详情。未确认原稿计入 beforeunload 保护。临时 Urgent 通道发送前另存 host-backed 原稿，确认 RPC 后仅清同一编辑版本；这不能被报告为持久消费或失败未知对账已经完成。

监督面板移除 1s 业务轮询，复用 workspace invalidation 的 60ms 合并/串行读取，并在显式切换监督 tab 时刷新。历史读取与 SSE 用 generation/revision 隔离，旧快照不能抹掉新回执。原 Requests/Reviews 测试夹具改为在数据变化时发宿主等价通知，未用加回轮询来满足旧夹具。Reviews 等其余业务轮询留待 DW-07 一起完成。

## 验证纪律

按用户最新要求，同场景收齐实现、失败分支与夹具后批量检查，尽量一次收集全部失败，随后只定向复测失败及受影响项。证据根 `daily-workflow-evidence/20261022`，前缀 supervision-input；当前集中检查退出码与日志记录在 REVIEW_HANDOFF.md。定向原生/客户端/L2 不能替代 L3 生产链、Linux、真实模型、持续运行或用户最终验收。无付费调用、push 或合并。

集中检查：Windows 存储 6、RPC 7、受影响 concierge 集成 6 与本地 provider 设置 1、客户端 8 均通过；workspace/all-targets clippy exit0，仅既有 27 lib 和 1 旧测试警告。L2 一次运行 25 项：23 通过、2 失败；两处旧夹具在首个状态到达前锁住历史读取，修正前置等待，同时验证慢历史读取不阻塞下一草稿。仅重测失败及受影响的 2 项，通过；没有重复跑已通过的 23 项。首轮失败与 trace 保留。

app/node 类型、定向 lint 和显式 VITE_MOCK=0 的正式前端构建 exit0；最终产物 supervision-input-production-final 的 HTML 网关为 grpc。截图已检查原 IDE 布局和新增 13px 状态栏；产物没有替换运行入口，不计 L3 或用户最终验收。初期夹具缺少业务通知的失败亦保留，详见 REVIEW_HANDOFF.md。
