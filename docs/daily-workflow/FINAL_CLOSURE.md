# 原计划与正式运行入口收口

2026-10-23。用户明确要求把本轮遗留工作全部完成、验收后推送。正式入口升级、原数据核对和当前任务文档一致性属于DW-09原范围，不再拆成“下一阶段”。本记录承接FINAL_REVIEW.md和REVIEW_FIXES.md；不改写历史验收原件。

## 范围核对

按根NEXT_STAGE_PLAN.md的G01～19、DESIGN.md和DW-00～09逐项核对现有实现、契约页和AC-01～30。此次没有新建业务执行器或扩大原方案。

| 原缺口 | 已实现边界与入口 | 对应证据 |
|---|---|---|
| G01/G10/G17/G19 | 主Chat保留会话；监督独立；宿主统一准入；新消息/Stop优先；直接run与聊天launch保留真实来源 | FOREGROUND、WORKFLOW_RECEIPTS、RESULTS；AC-01/10～16/26/30 |
| G02/G07/G15/G16 | 文本/画布/聊天/监督及位置的独立宿主恢复；CAS/durable ACK；孤立稿及晚到保护；跨副本回收不吃满配额 | RECOVERY、VIEW_STATE、SUPERVISION_INPUTS、REVIEW_FIXES；AC-02～05/16/24/25/28 |
| G03/G04/G14 | Send/Queue/Steer/审批/控制使用固定身份及可对账回执；未知不清稿；IME/重复按键守卫 | DELIVERY、LIVE_INPUTS、APPROVAL_DECISIONS、WORKFLOW_RECEIPTS；AC-04～06/15/23/29 |
| G05/G06/G12 | 明确引用已保存版本；保留原始UTF-8/BOM；三方比较、条件保存、create-only保留双方；回退失败可见 | FILES、STORIES；AC-07～09/17/24 |
| G08/G09 | 共享可读结果、准确原因/下一动作；结果进入草稿须显式Send；旧上下文具体提案确认后在原任务重新绑定，预算/消费/历史不重置 | RESULTS、REVIEW_FIXES；AC-10～14/17/30 |
| G11 | 共享事件流替代固定业务轮询；重连对账；备份写不触发业务刷新风暴 | EVENTS、PERFORMANCE；AC-18/19/28 |
| G13/G18 | 两条完整生产故事、四类事件源、断流/关闭/重启、获批真实模型、升级/冷恢复和候选交付 | STORIES、LIFECYCLE、REAL_MODEL_ACCEPTANCE、RESTORE_GUIDE；AC-20～22/27 |

旧任务记录中DW-02/03/04/05/07“尚未完成/不能关闭”、设计预览NOT_CREATED以及契约页的过期阶段描述已统一修正或明确标作历史。TASKS/ACCEPTANCE是当前状态，不用历史快照制造新待办，也不以改文档代替实现。

## 正式入口和保留数据

正式地址仍为 **http://127.0.0.1:19103/**，原生程序为`ff7b3463ee877e7914cdfa362acde34336c29694`，Web为`eed8e8ea5dafbf6e01f8575cd89e8dac944ed870`，包含本次更新提示修复。配置继续使用原本地受控provider，不新增付费模型调用。

证据根：`F:/space/06_Projects/Agent/daily-workflow-evidence/20261023/final-closure`。

- 原三个工作区先经真实RPC读取、再次比对并关闭，确认无活跃执行或dispatch；旧程序全部退出后才复制数据库。
- `cold-before-upgrade/`保存原个人data、原workspace、原试用workspace、原人工样本workspace及配置；`cold-backup.json`记录114个数据文件逐项哈希一致。
- `before-close.json`与`after-upgrade.json`逐项相符：原2个会话、5条scope关联恢复项、1个任务的完整预算/来源账本、1条执行记录及19个非内部文件。个人作用域记录可能出现在多个工作区查询中，5项不冒称5个不同正文。
- `provider-after-upgrade.json`证明切换与原数据读取为0模型请求。未重新seed原目录；合成验收在独立的新子目录执行。
- `ready.json`给出真实进程、端口、配置/数据和产品源码。`serve-formal-runtime.mjs`是保留原数据的启动脚本，运行`node <证据根>/serve-formal-runtime.mjs`；仅在该实例已停止时使用，不能同时打开同一数据库。

旧19103程序与冷副本保留，19104仍是修复阶段的隔离验收实例。新旧入口不能再混作同一运行候选。marker2回退限制继续遵守RESTORE_GUIDE.md。

## 正式入口补验

原未受影响L1/L2/Windows/Linux/真实模型/持续观察证据按ACCEPTANCE.md复用。本次补验用于证明升级后的正式入口和原数据可用，不新增费用授权或重复长时间观察。

- 配额回收及UI重新绑定：`production-browser-headless.log`中两项通过；真实Job在原任务只续跑一次，原草稿与完整账本保留。
- 两条完整故事：`stories-isolated.log`中的故事B通过；`final-production.log`中的故事A通过，实际修改共享函数、真实unittest、独立监督、返回原会话、人工脏稿与Agent修改保留双方均完成。故事A失败后的修复只复验受影响路径；未重复已通过的配额/重新绑定或原真实模型。
- `final-production.log`另含A/B会话归属与占用禁发、重开恢复通过，以及更新提示的首次安装/后续更新/可关闭/窄屏发送可点击/草稿保留通过；该提示用例显式替换service-worker事件，属于UI定向验证，完整故事仍使用真实生产栈。
- 此轮实际发现更新提示遮挡监督Send：首次service-worker接管被误报为“新版本”，右下角通知又没有关闭按钮。现仅对已有controller被替换报告更新，通知移至右上并可关闭；不强制刷新页面或清空草稿。已检查800×650截图，发送区没有遮挡。
- 升级快照工具通过真实前后对照；样本清单从ready读取实际产品源码，移除硬编码旧候选。
- 类型、定向lint、生产构建和diff检查通过；`artifact-manifest.json`冻结470项源码、270项Web资产和2个原生程序，较上轮产品源码仅AppUpdateNotice.vue变化。保留旧哈希资产以供已打开客户端使用；当前HTML只额外加入本地验收nonce。`final-preservation.log`证明全部补验后原数据仍匹配。

本次首轮问题完整保留：快照工具最初只列当前打开工作区，发现正式宿主重启后列表为空，改为显式打开三个既有工作区再读取；Recovery Read有record包裹，修正工具解包后比对通过。指定普通Chromium启动失败，使用已安装headless shell后可执行。人类试用seed的蓝图文件路径与自动故事夹具不一致，被产品身份守卫正确拒绝；已有Prompt和恢复位置也暴露故事脚本依赖空环境的假设，现按每条故事独立工作区并显式选择本次创建的有名资源。所有失败日志/trace保留；没有放宽产品守卫或把失败计作通过。

当前结论：原计划范围内的实现、正式入口升级、数据保全及交付文档已收口，无待开发或待切换事项。各功能独立提交；按用户本次授权快进更新fork现有Draft PR #9，远端HEAD和CI以证据根push-receipt.json及根CURRENT.txt的实际回执为准，不自动合并。

## 验收决定边界

原30项AC范围不删减。26项技术通过、AC-19/28经用户接受现有约6小时3分/353样本时长；AC-23/27的实际Windows IME和实际S3睡眠仍按用户先前明确的人工暂接受决定记录。自动化键盘和生命周期实现已有证据；未发生的物理操作不能代签为实测。这里保留的是验收决定与证据边界，不新增延期开发包，也不重启已获接受的观察或付费任务。
