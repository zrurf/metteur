# 文件协作的当前契约

2026-10-23，DW-04，基线9212d6a。保持原IDE布局及r2范围，主要操作13px，身份和长内容收进详情。

文件引用通过既有ReadFile的可选freeze_reference取得现有Version Flow快照、相对路径和SHA256；固定内容最多64KiB、16个、合计256KiB。发送及Queue/Steer从宿主快照取原始UTF-8内容，重新检查文件/历史读取权限，拒绝串工作区、篡改或缺失内容。Prompt与文件按用户选择顺序进入用户数据，不赋予系统指令等级。后续工具读取的是调用时磁盘内容。旧宿主未声明能力时拒绝静默降级。恢复的旧路径芯片必须重新选择。文件树和Composer共用已保存版/保存后引用选择，取消不发送。

冲突比较固定本地稿、共同基线和最新保存版。再次编辑或磁盘再次变化后原确认失效；保存仍使用精确expected_content/资源revision。覆盖前把已保存版留在既有Version Flow；Prompt沿用资源历史。Keep Both和删除恢复使用create-only，执行时独占创建，后来出现的文件不会被覆盖。Use Saved Version先等待原稿宿主ACK，保留该独立恢复记录，下一次编辑另建身份。

普通编辑支持UTF-8 BOM及一致LF/CRLF，两个编辑器保留原换行。混合/孤立CR只读，二进制不进入文本编辑。画布JSON仍走既有结构编辑与身份/mirror校验，不承诺格式字节不变。保存确认只更新对应版本，期间的新稿仍为dirty。

rewind/rollback固定原工作区，先保护当前文档草稿；失败呈现宿主错误，不冒充成功。Version Flow继续使用原预检、回退恢复快照和失败补偿；事件、授权、预算和输入回执不倒退。运行中保存不会暗中应用到运行图。

首轮实际文件/RPC、文件意图/恢复/蓝图历史和L2文件协作已通过，原始证据dw04-batch-*；协议消费者检查、生产链及最终跨包AC以REVIEW_HANDOFF最新记录为准。单项通过不等于用户体验或整阶段验收。

生产首轮发现protobuf JavaScript字符串解码会消费开头BOM；当前ReadFileResponse可选content_utf8携带确切字节，Web使用fatal且ignoreBOM解码。旧content字段保持兼容，固定摘要以原始字节为准。修复后的实际RPC与生产文件链已通过，证据dw04-bom-rpc-narrow及dw04-live-browser-bom。
