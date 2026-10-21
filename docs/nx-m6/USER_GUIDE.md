# Metteur NX 使用要点

## 项目记忆（Project memory）
在当前工作区查看笔记、约定和证据，按关键词与类型筛选。打开条目后核对来源、修订号、摘要及关联文件。Current 仅表示当前版本；不是正确性或人工审核结论。Pin as convention 会要求确认具体内容；模型意见不会自行成为用户约定。修改来源后，应重新审核，而不是沿用旧确认。

## Prompt resources
Project 与 Personal 是不同作用域，即使同名也不混用。明确选择 Add to Chat 或 Add to blueprint 后才参与上下文；个人库不会自动注入。运行使用绑定的内容快照，源文件变更后须查看新版本。删除源不等于删除已保存的历史快照。

## Task continuation
New task 中核对目标、模型、总轮数、总 token 和到期时间。Start task 会开始首轮；后续仅在归属明确、已保存的等待条件满足时继续。Pause 暂停准入，Stop 阻止后续自动继续，但不能撤销已经发生的文件或外部操作。Needs attention 应先核对结果与记录，不能盲目重跑。

## Schedules
计划绑定已有任务和会话。先选择保存的等待任务，再设置规则、时区、错过触发的处理方式；Review plan 后确认精确版本。改计划后需要新的确认。没有等待任务时，表单不会偷偷新建会话。

## 蓝图运行确认
Review a blueprint run 分为 Run overview、Bound versions、Conversation handoff。检查要做的工作、固定资源版本、原文读取范围和完成后的行为。勾选确认后方可运行；Decline 不创建该次蓝图执行。对话等待人工决定时应显示 Waiting for your approval。

## Addon pages / Preview inspector
安装声明页面的签名包后，从 Addon pages 打开。Page access 展示能力范围；插件只拿到隔离通信通道。停用、升级或关闭工作区会使旧页面失效。

示例检查器只读取宿主登记且由你选择的本地 HTML。源码检查完成后生成记录，再由宿主 Your review 接受或提出修改。接受成果不授予任何 shell、模型调用或文件写权限。

已补充可选的宿主离线 Chromium 检查器：真实视口截图、控制台捕获、横向溢出和图片 alt 属性检查。需要按 docs/nx-m6-completion/RENDERER.md 配置宿主；未配置会明确显示 not_configured。外部资源和网络被隔离，不能把有限检查通过理解为完整视觉验收。Model opinion 仍未调用。

## 当前交付边界
- 云端 Linux 本地候选；不是已经推送或合并的上游版本。
- M4 两类真实模型适配和对照测试已延后；没有真实供应商效果或成本提升结论。
- 新页面此前是暂时认可，本轮进行了技术与使用体验复核，最终美术定稿仍由用户确认。
- 1,000 节点、10,000 调用记录及 12 次数据库重新打开已做定向验证；原生 Windows、完整矩阵与多日压力运行未验收。
