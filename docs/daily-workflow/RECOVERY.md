# 输入恢复协议及当前接入边界

2026-10-23；当前正式接入文本、蓝图原始画布输入、Prompt 文档、主聊天/新聊天、监督输入及位置恢复。监督与位置的详细增量见 SUPERVISION_INPUTS.md、VIEW_STATE.md、WORKFLOW_RECEIPTS.md；完整恢复场景、Windows/Linux及全阶段技术AC已验；配额回收增量见REVIEW_FIXES.md，原数据升级见FINAL_CLOSURE.md。

主聊天在每窗口/实体使用独立输入副本；新聊天使用预分配 UUID，在宿主接收前不创建会话。每次 Send 另外保留点击时的固定输入；当前编辑稿与该原稿分别取得宿主 ACK 后才发起 Send。接收只清理匹配的正文/选择版本，拒绝不覆盖较新输入；未知保持原 pending ID，重开后仅对账。原稿保留到执行终态且源会话仍可读取；缺失来源保持可取回输入，不自动重建。恢复会重新核验 Prompt 当前权限及 revision/digest，不信任原 snapshot ID，不恢复 full/AllowOnce。

当前聊天/新聊天切换保留各自内存控制器，重开经 Recovery Copies 显式选择副本；新聊天原始稿可显式带入当前新草稿，已有会话的不同来源需打开对应会话或 Copy Text。未获 ACK 的本窗口输入可在恢复列表查看/导出；关闭页面的保护包含未备份聊天输入。没有持久浏览器正文缓存。监督普通消息与临时输入已接入独立原稿、pending身份与原ID对账，不与主聊天草稿混用；位置只恢复标签/选区/视口等，不恢复权限或正文。

## 存储与访问

`Recovery(ResourceRequest) -> ResourceResponse` 是增量 RPC，使用现有 schema 1 envelope。action 支持 capabilities、list、read、backup、discard、export；scope 为 project/personal。owner 从宿主认证身份取得；project 绑定规范 workspace root，personal 使用用户数据库。客户端不能提交 owner/permissions。RPC 受现有方法 ACL 控制；恢复正文只是输入，使用资源、模型和工具必须按当前配置与权限重新校验。

namespace 为 `dw/recovery/<SHA256(scope, subject)>/`，位于既有 EXECUTION_STATE；使用 sync WAL 原子批写。恢复内容不进入 append-only 资源历史、不广播业务变化、不参与执行 checkpoint 或文件 rollback。没有新增 column family；旧程序可以打开原数据库，但不提供本能力。未知 marker/schema 不自动降级写入；损坏记录隔离，支持有界原始导出。

单记录序列化上限 1 MiB；每 owner/scope 64 MiB，活动记录最多 512，含 tombstone 总记录最多 4096。已明确保存或丢弃的 tombstone 最多保留128条；超过后与本次写入一起原子回收最旧标记，同时持久化新的 creation_epoch。活动稿、损坏/未知记录不回收。分页最多50，只返回metadata，原文需逐条read。达到活动稿/字节容量上限仍保留现有副本和内存输入并返回失败。整库灾备回滚不在“已备份”保证内，应先导出最新恢复数据；业务rewind不影响本namespace。

首次回收将namespace marker升为2，记录正文仍为schema1。旧宿主拒绝读取marker2，不能原地降级；升级前冷副本/导出要求继续适用。新身份必须携带当前creation_epoch，旧身份的晚到创建请求不能复活已回收副本；已存在活动稿不受代次轮换影响。新客户端仅在首次写入明确拒绝且从未出现未知回执时读取能力、更新代次并创建新身份；未知回执保持原请求和原文，要求显式处置。丢弃重试遇retired表示宿主确认该已ACK身份已回收，可完成原丢弃；确认期间的新编辑仍另建副本。

## 版本与回执

每窗口新建独立 window UUID，每实体有独立 draft UUID。backup 使用 expected revision、严格递增 edit_sequence、request UUID。宿主持久提交后才 ACK。最近完全相同的请求可重复取得原回执；同 ID 不同内容、旧 edit、跨窗口/目标和错误 CAS 均拒绝。更旧请求不会被当成新稿覆盖。

客户端 750ms debounce，持续编辑最长 5s 发起一次备份，至多一个在途写；期间输入合并到下一次。未知结果保留原 request/内容重试，ACK 只确认提交版本。断网后新增内容没有持久浏览器缓存，显示未确认/未备份；重连先对账。

保存成功仅条件清理已保存内容对应副本。discard 需宿主持久 tombstone；正文与选项清除，但身份和拒绝晚到写的凭据保留。丢弃回执未知时保留输入；继续输入不会被该旧回执清空，确认后转为新副本。页面卸载/切页只 dispose 内存控制器，不表示丢弃。当前关闭脏标签需明确确认并取得宿主 discard 回执；失败或确认期间的新输入阻止关闭。

宿主确认文件 rename 后，原 owner 的文本/画布关联按目录边界原子转移，保留 previous_entity，旧路径晚到写拒绝。转移超限时保留原副本并如实报告“文件已重命名，恢复副本未转移”。其他用户、外部 rename 或已删除源不会按名称猜关联。

## 恢复操作

列表按窗口和来源呈现独立副本；选择后先预览，显式 Use This Copy。当前脏输入先取得备份，异步期间新输入阻止替换；恢复 fork 新身份，保留原副本。基线与当前磁盘不一致进入冲突，不能自动覆盖。源缺失仍可导出，不自动重建会话/运行。恢复无效 JSON/front matter 保留原始字符串，不以正式保存的格式校验截断。

UI 沿用已批准 r2 的 IDE 布局，恢复栏只展示状态与必要入口；ID、窗口、revision 和长错误收进详情。正式功能必须另做生产链和最终体验验收，隔离预览 fixture 的成功不算耐久性证据。

## 验证入口

- L0：app/node 类型、定向 lint，增量 proto 生成物和全部 Rust 消费者构建。
- L1：`cargo test --locked -p metteur-daemon --lib storage::recovery::tests`；`cargo test --locked -p metteur-daemon --test dw_recovery`。覆盖真实 DB 重开、文件/RPC、容量、损坏、写失败、作用域、晚到写、rename 和有界通知。
- 客户端：`node --experimental-transform-types --test scripts/test-recovery.mjs`，覆盖备份在途/未知、错误、fork、discard 在途/未知的新输入。
- L2：`E2E_PORT=19084 pnpm exec playwright test e2e/daily-recovery.spec.ts --project=e2e`，fixture 只证明 UI/控制器连接；另复测受影响文档套件。
- 原始命令、退出码、源码状态、首轮失败和证据见 REVIEW_HANDOFF.md。DW-03的当前L3已执行；DW-02恢复完整场景与L4/L5仍按阶段补齐，不能据此关闭所有关联AC。
