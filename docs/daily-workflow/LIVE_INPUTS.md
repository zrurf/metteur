# 主聊天 Queue / Steer 持久输入

2026-10-22；源码基线 5e7b81d 加本功能差异。只覆盖主聊天的 Queue / Steer；蓝图监督、Stop / 审批的持久回执继续实施。预览批准范围仍为原 IDE r2，默认只显示简短输入状态，新增文字约 13～14px。

## 接收与消费边界

沿用 ChatDelivery RPC 的 JSON 查询契约，新增 `live_submit {id,session,turn,mode,message,prompt_snapshots?}`、`live_body {id}`、`live_receipts {ids}`。能力为 `durable_queue=true,durable_queue_channel="chat"`；新 Web / CLI 必须探测能力，不降级到旧 Interrupt。旧固定目标 Interrupt 保留原临时语义。

接收在既有工作区 admission、当前目标及原 InterruptBus 下进行。正文（包括依次展开且重新校验当前权限/版本的 Prompt）、指纹和回执一次 sync WAL 落盘后才入队/ACK。指纹包含具体 session/turn、模式、原文与有序 Prompt snapshot IDs。相同 ID / 内容只返回原回执，异内容拒绝，结束后重传也不新建回合。missing read 后仍须以原 ID retire，关闭在途晚到请求。普通聊天的断流取消语义不变。

`live_queued` 表示宿主接收；根聊天 ReAct 在下一次模型边界先写 `live_claimed` 再加入上下文和显示历史。child / Job / 蓝图的旧 drain 不能消费根输入。根结束时先 seal 队列，阻止在保存窗口继续接收：未取走为 `live_unconsumed`；已取走且上下文保存成功为 `live_applied`；保存失败为 `live_unknown`。Applied 只表示进入持久上下文，不代表 provider 成功或要求已完成。进程丢失后的 queued / claimed 只对账为 unconsumed / unknown，绝不自动重放。

## 恢复、容量与兼容

复用 ComposerInput：提交前分别确认固定原稿及当前稿的 pending ID，接收后只清对应编辑序号/内容和选择；较新输入保留。重开恢复 Queue / Steer 原身份、模式及 turn，只查询/retire，不执行。独立原稿保留到 Applied 且来源仍可读；未使用/未知或来源删除时继续保留，用户可查看/复制。回执正文独立于恢复副本，删除恢复副本、rewind 和删除源会话均不删除已接收正文。

每 owner / workspace 仍为 8192 个回执、合计 16 MiB（含 live 正文），单回执 4 KiB、正文 1 MiB。满额/磁盘错误拒绝新输入，不淘汰正文；元数据更新也计入总额。原 bus 最多 256 个待取项，其中可靠输入最多 64；回执批读最多 64。schema marker 升为 2，后续手工 Send / retire 不降级 marker。旧宿主仅识别 marker 1，遇 2 保守拒绝；旧客户端不识别 live phase，保留输入未知，不解释成普通发送成功。

前端根据宿主回执显示排队、已进入本轮、未使用或未知，不以任意 assistant / tool 活动推断消费。复用 workspace invalidation 与 60ms 合并刷新，不新增业务定时轮询。切换目标后的晚到结果只修改所属输入控制器，不能改新会话。

## 检查与边界

证据根为工作区 `daily-workflow-evidence/20261022`，前缀 `live-input-*`；保留首轮失败与诊断 trace。Windows 原生存储 14、bus 6、RPC 最终 15 通过；客户端原稿/恢复/回执 32 项通过；L2 最终 3 项为实际 sendChat 回调驱动，受影响回归 23 项通过。类型/lint、workspace/all-targets clippy、正式前端构建通过，旧警告/具体日志见 REVIEW_HANDOFF.md。当前候选/完整故事与 L3/L4/L5 分开登记，不能以这些定向检查关闭整个 DW-02/03 或 AC。

首轮 host check 三处 Rust 类型错误修正后通过；客户端新增测试误用不存在的 restore 方法，改用已有 loadAsNewCopy 后通过。L2 初轮在切换连接后配置短暂加载时按键，诊断确认当时 ready=false；补齐就绪前置，未放松生产守卫。全 workspace clippy 初轮新增 bus 可合并 if 已修正；旧警告不归为新错误。无付费模型、push 或合并。
