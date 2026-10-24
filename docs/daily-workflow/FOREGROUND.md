# 主聊天、监督入口与前台占用

后续固定live-control目标增量见LIVE_TARGETS.md；本文件保留2a536f8当时的接入范围和未决限制。

2026-10-22；源码基线67e0578加本功能差异。WF-PV-01 r2已批准的入口与语义正式接入，保留原IDE布局。当前是DW-05的局部实现，不关闭整个工作包或关联AC。

主聊天始终绑定原workspace/session；蓝图运行时不再替换为Concierge。原始历史和草稿可读写，状态栏仅显示当前占用与Open Supervision、Pause、Stop。监督沿用Execution右侧独立面板，只有显式打开才定位对应run；完成不导航、不发送草稿。A/B切换分别保留正文和文件选择。运行身份与长诊断在详情，新增状态/操作13～14px。

ChatDelivery的增量admission查询在workspace admission锁下读取实际running/chats所有者，返回idle/chat/blueprint/blocked及对应run/session/turn。暂停、等待审批、正在停止都不视为空闲；仅执行所有者移除且没有未决后台写效果时返回idle。后台写效果复用SendChat的同一判定，未重写runner。读取不创建schema marker、目录或检查点，不发业务通知；未知格式/读取失败返回不可确认。既有服务不支持该查询时，新客户端保守禁用输入，错误详情可见。

客户端复用workspace事件刷新，合并在途查询；切换workspace、断线和晚到响应有epoch守卫，没有新增周期业务轮询。普通Send/Retry在执行前重查占用，Queue/Steer有当前会话占用检查；输入、快捷键、菜单和按钮同时受界面守卫。所有客户端的普通Send仍由宿主已有准入最终裁决。旧Interrupt协议尚无固定turn和durable回执，不能据此宣称Queue/Steer的RPC竞态、旧客户端绕过与控制Unknown分支已全部解决；这些继续归DW-03/05。

主栏Pause/Stop捕获明确run，沿用宿主run_id核验；Stop可以越过未返回的Pause/审批请求，不受共享busy状态锁死。完成旧控制请求不能清空新控制的busy状态。没有模型仍可直接操作；持久控制回执已由WORKFLOW_RECEIPTS.md所述增量接入；网络未知保留原身份对账，不称为成功。

Windows证据见REVIEW_HANDOFF与daily-workflow-evidence/20261022：真实DB只读/无迁移检查、真实聊天/蓝图暂停与legacy Send拒绝、L2 A/B、状态未知、全部输入路径、显式监督及Stop优先级。L3生产完整链、Linux/生命周期、监督草稿/位置、具体交接/结果和最终用户体验仍待完成。模型调用0，无push或合并。
