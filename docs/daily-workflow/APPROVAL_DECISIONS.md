# 具体审批决定与回执

源码基线 9273f92 加本功能完整差异，提交前摘要 approval-source-before-commit.json。本增量覆盖现有 broker 的审批决定、原 ID 查询/关闭及 Web/CLI 配套；Stop/Pause/Resume 的 durable 回执和临时蓝图输入继续 DW-03，不据此关闭整个工作包或任何 AC。

新增 ApprovalDelivery 只负责能力、describe、read 和 read_or_retire；RespondApproval 保留原窄操作，增量 target_json/content_digest 字段绑定实际 chat session/turn 或 blueprint run 和宿主不可变请求内容。新客户端先检查能力，旧宿主不静默降级。主聊天/Execution/Tasks 的审批共享同一请求 ID；从 Tasks 审阅不改当前聊天，直接 Stop 也绑定该审批原 turn。

broker UUID 同时是唯一决定身份。宿主 owner 绑定的 journal 在释放 broker/写权限前 sync WAL Claim；同 ID 的不同决定、内容或目标拒绝。相同已接收决定只返回旧结果，不再次授予权限。Claimed 或 Unknown 只查询/保留人工处置，不自动执行。作用已发生但最终 ACK 写失败保持 Unknown；不假称审批或后续工具执行成功。Workspace/Global grant 写入同步 WAL 后释放 waiter；该写入与项目回执不是跨库事务，崩溃窗口由 Claimed/Unknown 表示，绝不重放。

read_or_retire 与接收争用同一存储 gate。未接收身份持久关闭，晚到旧/新客户端回复均被拒绝；存在当前 pending 时先校验实际目标和内容。关闭该 waiter 只返回无授权的失败，不创建新的 runner、不恢复任何权限。只读回执不等待文件写锁。访问仍检查当前 RespondApproval ACL 和接收主体；身份总量8192/总16MiB/单4KiB，不静默淘汰。未来/损坏 schema 保留并失败。

前端提交前快照目标；匹配回执后才清当前审批。失败/未知保留内容与原身份，重复按钮禁用；显式 Check Decision Receipt 只对账/关闭该 ID。Pause/Stop 不受在途审批回复阻塞。错误采用13px必要说明，身份/长诊断不铺默认界面；保留现有IDE和模态结构。CLI手工、自动审批和TUI先describe并携带相同绑定，失败只查询原ID，不新增一次授权。

重开同一 pending 审批会先 describe 既有回执，旧Claimed/Unknown不执行；旧run消失后不能用同ID重建审批。跨重启的所有未决决定清单/分类结果入口仍待DW-06，生产链、跨平台、持续观察、真实模型和最终用户验收未执行。付费调用0，无push/合并。

首次集中检查：Windows回执存储5、实际审批RPC7、grant存储3、原approval_scopes21、客户端8通过。workspace/all-targets clippy exit0，新增4处可合并if已修正；前端app/node类型和lint exit0，两个变量遮蔽警告已修正。L2 16中14通过/2失败，新审批7和前台隔离6均首轮通过；另6项受影响监督/审批通过。两个旧续聊夹具未等恢复就发送、又将所有资源请求误认为续聊查询，修正夹具后仅失败2项窄复测通过。未重跑其余已通过场景。

R-batch真实RPC/本地provider集成首轮失败，暴露执行列表将dw审批回执误解析为checkpoint；同时核验dw草稿和聊天投递命名空间。修复保留16字节UUID的实际checkpoint和损坏checkpoint报错，新增实际三类存储共存检查1项通过；仅R-batch窄复测1项通过。daemon lib最终clippy exit0，仅既有27项警告，未重跑已通过的存储/审批/权限场景。首轮源码摘要approval-batch-source.json、失败日志和trace保留，native窄复测源码见approval-narrow-native-source.json。

正式前端显式VITE_MOCK=0构建exit0（approval-production-final-build.log），HTML metteur-gateway=grpc；没有更新运行版，不计L3。前端变量改名/夹具修复后app类型及lint exit0。各轮不累加为AC，模型费用0，无push/合并。

DW-07继续核验运行内approval_details缓存回收及前台等待原因以实际pending IDs派生，避免已接收审批后遗留状态文案；这不改变broker已消费请求和持久回执事实。
