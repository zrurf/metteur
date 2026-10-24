# 当前候选试用入口

2026-10-23正式入口已升级：原生采用ff7b346，前端包含本轮更新提示修复；准确源码/产物与进程见final-closure/ready.json及[FINAL_CLOSURE.md](FINAL_CLOSURE.md)。原会话、任务账本、恢复稿及试用目录保留。保留已批准IDE布局和字号；原28项通过、2项人工暂接受决定见ACCEPTANCE.md。

当前程序在 **http://127.0.0.1:19103/**。这是生产Web、网关和Windows daemon，配置的是隔离的本地脚本provider；真实文件编辑、Python测试和解释器均实际运行。该入口用于操作/体验验收，不能评价真实模型理解能力，不产生付费模型调用。

原数据经过关闭/冷备份后升级，逐项RPC比对通过；当前运行清单为20261023/final-closure/ready.json。正式入口补验使用独立工作区，不重写原试用目录。升级过程零模型调用；后续生产故事仅调用本地受控provider。

已准备全新试用工作区：

`F:/space/06_Projects/Agent/daily-workflow-evidence/20261022/s4-user-trial-ccecd9c/workspace`

打开[试用工作区](http://127.0.0.1:19103/?open=F%3A%5Cspace%5C06_Projects%5CAgent%5Cdaily-workflow-evidence%5C20261022%5Cs4-user-trial-ccecd9c%5Cworkspace)。样本/资源/已保存蓝图身份及原始目标见同父目录`trial.json`。准备样本没有启动模型或运行。

## 故事A：修复并验证

1. 在Explorer打开`shared.py`，留一条未保存说明，等待Backed Up on Host。右键该文件，Add to Conversation，明确选择已保存版本；用户草稿继续保留。
2. Prompt Library里分别选Personal和Project下的`Trial normalization rules`，Use in Chat。两个作用域同名，各有自己的身份和版本。
3. 在主Chat发送：`DW_STORY_A: 修复两个调用方共用的归一化，实际验证并返回这里。`
4. 审核文件修改及具体蓝图版本/交接/结束策略，再确认运行。Python命令为`py -3.12 -m unittest test_shared.py`，样本留有8秒观察窗口。
5. 从Tasks的Open execution进入独立Supervision；可发送`DW03_SUPERVISION_MESSAGE: 根据宿主记录报告进度，不要修改文件。` 主Chat保留原会话。后台续聊提出审批时，从Tasks的Review requested action检查固定目标。
6. 查看执行、确定性检查和文件差异。人工脏稿与磁盘修改冲突时，Compare Changes后选择保留双方或明确保存，不直接覆盖后来版本。模型意见保留来源，不能作为测试或人工通过的替代。

## 故事B：同一任务的两个阶段

从Tasks → New task，选择配置模型`fixture`，确认目标/期限/上限。目标：

`DW_STORY_B_STAGED: 先实现筛选并验证，再加入limit并验证两阶段，保持同一任务和预算。`

第一阶段运行`test_stage1.py`；第二阶段运行`test_stage1.py test_stage2.py`。每次文件/蓝图审批分别审查；后台审批从Tasks打开。完成后两个blueprint和两个Job来源都保留，累计预算不会按阶段归零。样本首次操作后文件已修好，重复试用应准备新的隔离样本，不能把重复运行当作一次新修复。

## 恢复、控制与结果

普通聊天、监督及文件输入的备份分别归属原会话/run/路径和窗口。重开先恢复位置，正文从Recovery Copies明确预览后Use This Copy；当前磁盘变化会先提示冲突。未确认输入如实显示未保护。不要把恢复输入当作恢复权限或重新执行。

结果分执行状态、宿主检查、人工判断；原始输出在详情，缺检查不等于通过。Add result to draft和Prepare new plan只追加当前稿，明确Send才进入新模型回合。未知效果/用量不能用通用Continue清除。Stop固定原task/run目标；停止和历史记录不随文件回退消失。

普通聊天断流会取消并收尾；明确获批detached才按保存政策继续。活跃聊天/蓝图占用时关闭工作区会被拒绝；等待中的后台来源在关闭时收尾并暂停自动续跑。实际daemon退出后的未知Job不会按旧PID重放。机器睡眠期间不保证触发准点。备份/升级/回退按[RESTORE_GUIDE.md](RESTORE_GUIDE.md)，**R8原地打开新数据目录不受支持**。

## 人工体验反馈

建议实际试用1–2小时，记录开始/结束时间、上述两个故事、双窗口/刷新、窄屏/键盘/明暗主题，以及字号和默认信息密度。真实Windows中文输入法中，候选确认Enter不能误发送；Shift+Enter换行、长按/重复按键不能重复提交，运行占用不能从快捷键绕过。睡眠/唤醒项由用户在方便时实际操作；未做就注明未测。

可按这一行反馈：`final-closure/ready.json对应候选，实际时长…；故事A/B…；布局/字号/信息密度…；IME…；睡眠/唤醒…；问题或未测…`。r2已批准，不重复要求批准原设计；这里只收具体候选的最终体验结果。真实模型任务及新预算方案另见REAL_MODEL_ACCEPTANCE.md。
