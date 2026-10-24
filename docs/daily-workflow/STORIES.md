# 两条生产工作流的验收记录

2026-10-23。证据根为工作区外 `daily-workflow-evidence/20261022`。本记录是生产 Web → grpc-web 网关 → Windows daemon → 本地脚本 provider 的 L3 结果；文件、进程、解释器和确定性检查真实执行，脚本 provider 不代表真实模型理解能力或用户体验签字。

故事 A：在原 Chat 显式选取同名 Personal/Project Prompt 和脏文件的已保存版本；ReAct 使用真实 SearchProjectMemory、ReadFile、WriteProjectNote 和 EditFile；笔记保留 run 来源、三个文件绑定、model_opinion 与空 confirmation。用户确认保存蓝图的具体版本与回原会话策略后，解释器执行真实 Python unittest 并通过 Validator 记录退出码。运行中进入独立监督查询，原会话不变为监督。完成后用户草稿仍可比较，Keep Both 保留人工副本与 Agent 磁盘修改，两个调用方未变。`s4-stories-browser-fixture-fix` 中故事 A 通过，12.7秒。

故事 B：一个明确授权的 Task 先实现筛选并验证 stage1，再在同一父任务续聊加入 limit 并验证 stage1+stage2。两次启动都实际经过文件审批与精确蓝图交接确认，后台审批从 Tasks 的 Review requested action 打开。最终两个 blueprint 和两个 Job 来源均 succeeded，两项当前 Validator 检查通过；used_turns=3、max_turns=4，累计消费不清零，重读保留预算及全部来源。完整操作在 `s4-story-b-review-retest` 执行；失败的同名旧任务选择断言已按唯一 task ID 补验，见 `s4-story-b-exact-target-retest.json`。未重新执行已通过的两阶段工作。

首轮失败和处理：`s4-stories-browser-first` 是夹具 DSL 字符串中双引号的解析错误，改为编译图后设置原生 Validator 配置；`s4-stories-browser-fixture-fix` 的故事 B 未点击已有的后台审批入口，补齐操作；第二次故事 B 操作完成，但结果查询误选已停止的同名旧任务，改为捕获新任务 ID 并只补验后置断言。全部原始 trace、截图及旧任务记录保留；不能称首轮全过。

验收发现状态文案缺陷：续聊仍保留旧 wait 时，Active 被描述成等待来源，后台待审批也未投影为明确下一动作。当前修复只读取宿主实际状态和 pending approval；不改变调度、审批权限、代次、根预算或事件消费。权限/未知效果/预算/过期守卫优先。Windows 和 Linux 的原因矩阵及来源引用检查针对该变更定向执行。

同批 Linux 首轮真实 DB/RPC/文件契约已通过：chat_rewind、dw_delivery、dw_files、dw_recovery、dw_results、nx_final_boundaries、recovery_admission。`s4-linux-event-sources-first.log` 的 Job、只读 child、取消未知消费、持久 timer 与失败批回执5例全部通过。Windows 文件链、只保留结果策略和当前未改动控制边界分别复用190b48d、64d464f及9212d6a的定向证据。

无结果的“准备新计划”已补入现有任务详情，只追加当前草稿中的任务来源/目标/原因；不续跑、不重置旧预算。`s4-plan-entry-browser.log` 定向通过。默认 IDE 几何未变，必要动作与主文字保持约13–14px，长信息折叠。

上述后续工作现已按ACCEPTANCE.md收口：生产恢复/生命周期/固定性能/模型/用户试用有对应证据，持续时长与人工操作边界保留用户实际决定。正式入口升级后的增量复验见FINAL_CLOSURE.md。
