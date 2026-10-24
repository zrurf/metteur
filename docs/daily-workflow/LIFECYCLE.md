# 关闭、断流和实际重启验收

2026-10-23。当前增量在`s4-lifecycle-runtime-r1`冻结，Web复用eeb930c未变化部分。生产入口19093，本地脚本provider；不代表真实模型评估。源码/二进制/Web对应候选由manifest.json记录。

验收修复了两项真实问题。第一，旧grpc-web和SSE转发层只在下一条事件到来后才发现客户端已关闭；普通聊天断流后，延迟返回的工具调用仍修改了合成文件。`s4-disconnect-effects-before-fix`保存UNWANTED_AFTER_DISCONNECT原始失败。两层现均同时观察接收端关闭并及时释放daemon观察流。宿主仍按普通聊天取消、获批detached继续的既有政策执行；取消时保留在途上下文和使用量，不将未知值改成0。

第二，关闭工作区杀掉Job后，其终态观察者仍短暂持有数据库，Close ACK先返回，立即Open因RocksDB锁失败。现将既有Tokio后台来源回执任务纳入工作区收尾；暂停政策/取消Job后，等待Job及只读child的回执任务结束，再确认释放。没有新执行器或账本，Stop代次与根预算未变。观察者异常导致关闭失败可见，不提前返回成功。

生产`s4-lifecycle-production-fix`通过：普通聊天运行中Close被拒绝；gRPC及浏览器SSE断流后，合成文件保持原文，同ID重传只返回原receipt；获批detached等待时关闭工作区，立即重开成功，宿主ACK草稿和累计预算保留，未自动dispatch。`s4-detached-reconnect-first`通过：没有原观察流时，真实8秒Job只在同一任务/会话续跑一次，单一source、单一committed dispatch、used_turns=2，实际本地provider共3次请求。

`s4-daemon-restart-unexpired`执行了实际进程退出：严格核对隔离daemon的PID和冻结exe后退出，保留原数据、日志和配置；原路径重启，未过期政策下Job标indeterminate，主机事实为effects_unknown，原消费/代次/ACK保持，Resume拒绝，新provider零请求且没有重建Job。内部state可能先为ready，表示来源已到齐；allowed_actions与effects_unknown守卫仍要求人工，不能理解为可执行。首轮重启夹具政策窗口过短且错误要求特定内部state，原失败保留，延长合成授权窗口后只复测该场景。

Windows/Linux真实Job关闭后立即重开通过；只读child取消的未知预留/用量也保留并可立即重开。子代理首轮夹具没有使用已验证的模型别名/默认窗口预算，未进入在途调用；已按既有NX样本修正为受控模型及充足预留，原失败全部保留。web-server原有7例与daemon/web全部目标clippy通过；仅既有27 lib及1旧测试告警。Linux受影响Job/child/源事件5例通过，未重复未改动文件/结果全套。

`s4-session-ownership-first`首轮通过：A实际交接后主动选择B，普通Enter在占用时保留B草稿；完成不抢页面、不把主Chat替换为监督、不多发模型回合；新浏览器页取回B位置，经明确恢复取得B独立宿主草稿，历史未插入未发送内容。

没有实际让用户机器睡眠。IANA时区/DST/misfire由既有确定性时钟契约覆盖。2026-10-23用户决定将真实IME及实际睡眠/唤醒暂接受、后置补验，不阻塞本轮交付，不再催验；清单和样本保留于MANUAL_ACCEPTANCE.md。实际操作证据仍缺，不将暂接受写成实测通过。持续观察只报告实际时长，不将上述短流程称为8小时通过。
