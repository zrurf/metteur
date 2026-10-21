# 宿主工具边界
模型换型、Prompt文本、工具JSON schema均不能改变宿主权限。

| 角色/入口 | 本阶段规则 |
|---|---|
| 主执行ReAct | 保留现有工具权限与沙箱；RequestBlueprintRun只对depth=0的前台会话开放；AwaitEvents/StartSubAgent/ReadTaskSource还需宿主续跑能力 |
| 蓝图执行器 | 同一工具能力判定，不能通过Tool节点绕过角色限制；启动另一蓝图的前台能力不下放 |
| 同步SpawnSubAgent | 保留既有命令/文件操作及共享审批，不混同异步只读任务；不继承当前聊天的显式Prompt选择，不能写项目约定 |
| 异步只读子任务 | 宿主read_only_tools标志跨嵌套保留；固定只读清单与已有allowed_tools取交集；模型传入allowlist也不能授权写入/启动新任务 |
| 客服 | 使用原有tool-free模型通道、读取与转交；不能调用通用registry工具 |
| 监督 | 使用原有ReadBoard/ReadBlueprint/ReadRunStats/受控提案与审批路径；ReadHandoff仅当前run有绑定时提供，不变成通用执行者 |

新增查询：
- ReadRunEvidence：仅当前蓝图或根任务绑定的run；状态、版本、in-flight和最多64条检查；输出上限16KiB。没有调用方任意run/session参数，不把执行成功等同用户验收。
- ListPromptResources：最多20条当前项目元数据及当前明确选择快照的引用；不列举未选个人资源、不自动选择或应用；16KiB。
- ReadSelectedPrompt：只能读取宿主当前聊天回合明确选择或当前确切蓝图launch绑定的快照；同owner的其他快照也拒绝；36KiB，用户级内容不是授权。
- 原有记忆/等待/子任务/Job/蓝图请求继续复用，不新增第二套命令执行器或模型授予权限的入口。

新增通用限制同时接在ReAct广告目录、ReAct实际调用和蓝图Tool执行入口。监督和客服继续独立专用调度，不借用通用入口。工具自身的存储owner、资源revision与源绑定检查仍保留，不靠提示词作为安全边界。
