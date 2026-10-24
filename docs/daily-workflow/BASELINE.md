# DW-00 实施基线与契约冻结

2026-10-22，Asia/Shanghai。模型调用 0。此文是实施记录，AC 通过状态另行登记。

## 工作区及交接差异

- 唯一活动仓库：`incoming/metteur-cloud-ff7b795-20261015/repo`；根 Agent 属于上级 F:/space 总仓库，不在该总仓库提交。
- 起点：`6b19ffa36ea1013c8cd4eb14f49e5e32590081b1`，原分支 `codex/workspace-reliability-20261021`；已新建 `codex/daily-workflow-20261022`。既有四份 local-handoff 修改、DESIGN/TASKS 均保留。
- GitHub connector 实时核实 PR #9 为 OPEN/Draft/unmerged，head 同起点；base `503299f59df57ff09a43aa264bd8a0727d18aee0`，共同祖先 `fb3bb9fa985b5f9a32a7064e994e1801ff4850aa`。不需要丢弃或重放 R8 修复；不修改该历史 PR。
- 服务实际未运行：1801 无监听，Metteur 进程清单为空。R8 运行版及启动入口保留。新的 mock 复现用 19080，预览用 19081，后续生产 fixture 用 1802（启动时再核实）。
- Windows 保留 TCP 5284～5383，首次 5319 监听被 EACCES 拒绝；不通过改系统保留规则解决。使用未占用的 19080/19081。
- Node 24.13.1，已安装 pnpm 11.25.0，锁文件声明 pnpm 11.24.0；不改锁及依赖。原生 Rust 使用已保留 VsDevCmd/LLVM/protoc/cargo 缓存，独立 target 重建中；WSL Ubuntu-24.04 存在，Linux 当前候选未测。
- 无适用 AGENTS.md（检查从 F:/ 至活动仓库父目录及仓库文件清单）。默认单代理串行。

## G01～G19 定向核验

源码风险与运行观察分开。L2 是当前 R8 产品组件与 mock gateway，不能证明生产宿主行为。

| 缺口 | 当前事实／结果 | 下一验证 |
|---|---|---|
| G01 | L2 激活 execution snapshot 后出现 Run concierge，主 Chat 被替换，已复现 | 双 session/run、生产准入 |
| G02 | 切 Tasks 返回 Chat 草稿实际保留；documents 的 Map 无持久化，刷新保证尚缺 | 四类 ACK/重开/磁盘变化 |
| G03 | L2 sendChat 明确拒绝后正文为空，已复现；附件在 send 前被移除 | 接收前后与保存中继续编辑 |
| G04 | SendChat 无客户端投递 ID；控制 UI 先清 approval/停止状态，源码风险 | 接收/查询崩溃窗口、控制失败 |
| G05 | 普通附件仅拼接 @file 路径；现有路径语义不冻结内容 | 宿主校验固定身份 |
| G06 | L2 rollback 返回失败时 version.error 仍为空，已复现；现有文件保存有 CAS | 三方比较、再次变化 |
| G07 | restore 选 sessions[0]；tabs 与位置内存态 | 最近位置/指定身份缺失 |
| G08 | TaskPanel 主要 JSON；缺共享结果投影 | 准确来源与检查链接 |
| G09 | 控制支持 pause/resume/stop/cancel_schedule；NeedsAttention 已拒绝通用 resume | 分类窄动作，不移除守卫 |
| G10 | continuation 已有 revision/generation/Stop 守卫；未复现宿主绕过 | 受控迟到事件验证 |
| G11 | ConciergePanel / OversightReviews 仍固定 1s/1.5s 读取，源码确认 | 持久提交通知覆盖与请求测量 |
| G12 | 原有 checkpoint/rewind/文件恢复可复用；新恢复记录必须独立 | 脏稿、手工改动和失败注入 |
| G13 | 本阶段模型证据与用户验收均未产生 | 准备任务后集中申请额度 |
| G14 | L2 isComposing=true/keyCode229 的 Enter 清空输入，已复现 | Windows 真实 IME 用户操作仍待测 |
| G15 | 现有内存草稿没有 durable discard tombstone | 丢弃晚到/rename/孤立原稿 |
| G16 | ResourceStore 每写追加 history/requests，不宜用于高频恢复备份 | 专用有界 namespace |
| G17 | start_chat_turn 已通过 ws.admit、running/chats、未决后台写检查 | 启动/暂停/审批/旧客户端真实链 |
| G18 | 普通 stream 断开与 detached 已有不同路径；不预判守卫失效 | 生命周期矩阵，各项实测 |
| G19 | ExecuteBlueprint 与聊天 launch 两入口存在，前者不含 source session | 不伪造来源/续聊 |

原始结果：工作区外 `daily-workflow-evidence/20261022/baseline-second-retest/baseline-browser.json` 与截图。前两次脚本因相同 aria-label 的新组件匹配失败，日志/JSON 保留在 baseline-first、baseline-retest；修正观察定位器后复测，未改产品预期或实现。`scripts/dw-baseline.mjs` 是基线观察工具，不作为未来修复通过测试。

## 最小接口、存储与兼容约定

1. 恢复使用增量 `Recovery(ResourceRequest) -> ResourceResponse`；不复用 append-only ResourceStore，不进入 Version Flow/checkpoint。owner 来源为认证 subject，scope/workspace 由宿主解析；personal 使用现有用户 DB。项目恢复仍区分 subject。正文、附件及 Prompt 仅作为输入数据，不能包含可恢复的授权。
2. 草稿 schema 1：独立 UUID draft ID/window ID、entity kind/identity、record revision、edit sequence、原始 JSON 字符串（内部允许无效源文）、基线身份、有序引用、pending submission identity、更新时间。scope/subject 不接受客户端 owner。record CAS 与最新请求指纹原子 durable；同 ID 异内容拒绝、旧 revision/sequence 拒绝。明确丢弃为永久轻量 tombstone，旧 ID 不再启用；新编辑另取 UUID。
3. 最大单记录序列化 1 MiB；单 owner/scope 64 MiB，最多 512 个未解决副本及 4096 个含 tombstone 的记录。超限拒绝且保持原件，不截断、不自动按年龄淘汰；容量不足由用户明确导出/处理。单页最多 50 个 metadata，正文按 ID 单独取，响应分页目标 ≤256 KiB。编辑初值 debounce 750ms / 最长 5s / 每稿单在途、合并待发。
4. 每稿只保留当前正文及最新请求回执。旧写重传返回 stale/revision conflict，不会作为新写；丢弃有 tombstone。schema marker 与记录不认识时 fail closed，损坏记录逐条隔离呈现，不自动删除。备份原子 durable 成功后 ACK；备份不会广播工作区业务失效通知。
5. 发送在现有 runner 前补 durable 接收记录与查询，不另建执行器。投递 ID 与输入指纹、session/turn/checkpoint、revision 原子接收；已接受后的断流仍按普通/获批 detached 契约处理。接收未决关联恢复记录，unknown 禁止重新投递成新 ID。
6. Send/Queue/Steer 及控制目标保留明确 run/turn/session；同逻辑 ID 使用同记录，异内容拒绝。轻量去重凭据不静默淘汰，达到 scope 容量后明确拒绝新请求。receipt 查询有权限和分页。具体 durable 事务边界在 DW-03 实现记录补实测。
7. 保持 16 MiB RPC 总上限。恢复 envelope 自己限 2 MiB（JSON 转义膨胀后仍受记录序列化 1 MiB 限制）；现有 resource decode 的 64 KiB 不直接套用草稿。普通文件当前仅 UTF-8/传输限制，超过恢复上限明确未受保护。
8. 新客户端连旧宿主：Unimplemented 显示能力不足，不声称已备份或已接收；旧客户端请求经过同一准入但不宣称新幂等保护。旧程序可打开未新增 CF 的旧 DB，独立 namespace 保持不干扰；旧程序不会维护新恢复能力，回退先导出未解决记录，整库恢复不保证新稿。添加协议时同步生成物/网关/mock及必要调用方。

## 性能与定向验证约定

初次 dev/mock 启动至 Chat 可用实测 3431ms（首次 Vite 优化，非生产承诺）。后续重测固定机器、1360×900/单 Chromium/同 2 文件样本，分开报告冷/热启动、切页、编辑、网络、存储增长及持续运行。初值预算：热切页 p95 ≤ max(500ms, baseline×1.25)，按键响应 p95 ≤ max(100ms, baseline×1.25)，生产冷启动 ≤ max(5s, baseline×1.25)。冻结样本测量完成前不宣布 AC-19 通过，若初始样本需要调整须保留原因与旧结果。

固定监管 10 秒空闲区间：目标无 1s/1.5s 业务查询，连接保活和本地时钟单列；1000 次持续备份：同稿业务行数恒定、不广播全页查询，DB 物理 compaction/WAL 增长另测，不声称磁盘字节绝对不变。拒绝/未知/容量/故障不应显示成功。

旧 908/921/138 仅用于了解未影响能力；本阶段 AC 全部从 NOT_RUN 开始。完整故事用隔离合成项目，不读取生产凭据。付费模型 0，新额度/允许数据在准备好任务后单独获取。预览 r1 获批前不接生产入口。

2026-10-23实际回退核验补记：R8能读文件/会话，但无法原地列出含新增dw namespace的运行记录；原假设不能当作已验证兼容。当前版前进读取R8及升级前冷副本回退/返回当前已用真实二进制执行，新稿和后续改动分别保全，见RESTORE_GUIDE.md与ACCEPTANCE.md。基线原记录不删除。
