> **2026-10-24 最新交付：编程工作流 WD-00～07 已完成。** 四类模板、逐项验收、三模式真实对照及正式 19103 升级已收口；当前版本、证据和推送回执见 [最新交接](../workflow-delivery/REVIEW_HANDOFF.md)。下方状态仅代表原历史时点，不重新激活已完成任务。

# 实施对话给规划复核对话的交接

最新收口：用户要求完成原计划全部遗留并验收后推送；正式19103已升级、原数据核对完成，当前源码/运行及完整范围索引见[FINAL_CLOSURE.md](FINAL_CLOSURE.md)。下方旧19103、未升级、仅讨论及旧HEAD描述均为历史快照，以最新收口记录和根CURRENT.txt为准。

更新：2026-10-23。活动分支 codex/daily-workflow-20261022；起点 6b19ffa36ea1013c8cd4eb14f49e5e32590081b1。本文件反映实际状态，不代替用户批准或完整验收。

## 交付历史（当前状态见顶部最新收口）

2026-10-23复核修正：原交付2608bb9已分六批快进推送fork/codex/nx-final-acceptance，现有Draft PR #9同步完成；旧“fork head6b19ffa、待推送”是推送前快照。本轮用户授权直接修复配额回收、旧上下文重新绑定和文档状态，随后要求“确定完善后推送,然后我们讨论下一步方案”。功能ce3e5da / ff7b346及验收文档已快进推送同一fork分支，保留Draft PR，不合并。进度/证据统一见[REVIEW_FIXES.md](REVIEW_FIXES.md)，最终远端回执见根CURRENT.txt和证据根push-receipt.json。本轮定向验收及两条生产链通过，修复版入口19104（独立合成数据）；19103仍是旧冻结产物。当前接续为下一阶段方案讨论，不自动开始新实施。

以下是原交付的按时间倒序历史：

- 本轮最终验收与推送授权：用户要求“最后验收然后推送上去”，获知原观察退出／实际约6小时3分后明确“记作通过,时间已经够了”。AC-19/28据此通过，AC-23/27仍暂接受后续人工补验；本轮28项通过、2项暂接受，无待输入门槛，不称满8h或30项均实测。最终产物556源码（195前端／361原生）、270资产、2二进制零差异；19103同一冻结程序／UI／原数据库已恢复，实际网关ListWorkspaces成功，模型0请求。详见FINAL_REVIEW.md和s4-final-review-20261023。既有PR #9仍OPEN/Draft、fork head6b19ffa，按本轮授权分阶段快进更新该PR，不推origin／强推／自动合并。下文运行中与未授权均是此前快照。
- 最新用户决定：“这些能不能待我验收,不行的话先暂定通过,后续遇到问题再让你修复”。AC-23/27改为暂接受／待补验，不阻塞本轮交付，不再等待或催促人工操作；保留真实IME/实际S3睡眠尚无证据的边界，后续问题按场景修复。当前26项技术通过、2项人工暂接受、2项持续观察进行。决定及当时状态保存在s4-remaining-acceptance/user-deferral.json；本轮仅更新文档，不重跑任何已验场景，不新增模型调用。
- 剩余观察及后续人工样本：MANUAL_ACCEPTANCE.md与s4-manual-r1专用工作区（两个仅Delay蓝图，准备前0运行/任务/聊天）。系统S3/电源日志读取能力、PID创建身份已只读记录。原8h观察不重启；只读汇总器exec86907在s4-remaining-acceptance生成统计，完成后才在样本文件树写“可以开始睡眠验收.txt”。本次决定不触碰原Node54492/汇总器/运行产品，不重跑业务/模型/基线；此后旧段落中“人工待收”按当时快照理解。
- 最新：用户明确批准UI-CONSISTENCY r1接入，生产前端65cc92f已在19103接原daemon61576/原数据，原生ccecd9c不变。s4-ui-production-first真实控件/宿主ACK/长标题/窄屏暗色首轮通过，未调用模型、未创建任务；清单s4-ui-integrated-r1。AC-21技术通过，当前26项齐备，剩AC-19/23/27/28。19093保持原观察，不能把新Web进程称已长测；下文“r1待确认”是批准前快照。

- 最新人工反馈“暂时没问题”，随后要求统一页面主标题与控件细节。UI-CONSISTENCY r1已完成共享PageHeader/18px标题/一致位置、任务标题操作、执行/设置共享控件，未加UI依赖；隔离冻结预览19101，详见UI_CONSISTENCY.md。当前19093/ccecd9c与持续观察未替换，当前工作树/后续UI提交不再等同其冻结网页源码；新增可见差异按C14待具体版本确认后接入。IME/实际睡眠/时长不由一般正面反馈代签。
- 最新人类答复已批准新增US$1/最多36请求，并开始试用、稍后反馈。当前候选三项L4首轮全部通过，14请求全部settled、峰值费率估算US$0.0298446（provider实际金额null），未重跑。源码前后不变、二进制对应ccecd9c；证据s4-real-model-ccecd9c-first及s4-real-model-result.json。AC-20技术通过，当前25项齐备，剩AC-19/21/23/27/28；此前下文“额度0/待预算答复”是授权前快照。持续观察及实际人类结果仍未代签。
- 当前产品候选ccecd9c，运行19093；s4-lifecycle-runtime-r1/manifest.json已对应产品提交，运行源码零差异（测试夹具修正另记录）。真实断流后修改文件、关闭ACK先于DB释放两项阻塞已修复；原失败、gRPC/SSE两桥、Job/child取消/实际重启证据见LIFECYCLE.md。s4-session-ownership-first首轮通过原A启动后B选择/占用输入/完成不抢页/新页位置与草稿恢复。
- 当前技术矩阵见ACCEPTANCE.md，24项技术证据齐备，剩AC-19/20/21/23/27/28明确保留。8h双窗口观察在s4-observation-ccecd9c运行（launcher exec22384、Node54492），不提前宣称完成；旧eeb单窗实际71.2分钟/71次ACK后停止，数据保留。新预算US$1/36请求及用户实际试用反馈仍待人类答复，付费调用0。
- Windows真实R8与当前程序：原地向后打开新增namespace的ListExecutions失败，原记录保留；前进升级/升级前冷副本回退/返回当前与双方新内容保全已实测，见RESTORE_GUIDE.md。旧冻结二进制摘要已核对，没有修改原运行包。完整人类试用样本已准备，DAILY_USE.md列入口和操作；未启动试用模型。
- 本地完整提交ccecd9c（生命周期修复）、eeb930c（共享连接）、b6a451d（两故事）、14e9aba（新预算守卫）、7cdeb8e（恢复/持续验收工具）。无push/合并。下文历史“待补/未提交/运行中”按其当时快照保留。

- S4两条真实生产故事已执行，详见STORIES.md及s4-stories-*证据。故事A首个实际流程通过；故事B后台审批入口和同名旧任务定位的夹具问题分别修复，实际两阶段已完成，仅按唯一task ID补验失败后置断言，不重跑真实测试。两次蓝图、两个受控Job、3个累计turn、原根预算及全部来源保留。
- 无source的新计划仅追加草稿入口L2定向通过；生产验收发现后台待审批被写成等待来源，已改为读取实际pending approval，并修正Active沿用旧wait的文案。Windows/Linux原因矩阵和结果引用定向通过；不改变调度、权限或预算。此增量的Windows生产程序正在构建，尚不将19090旧程序称为新修复运行版。
- Linux同源码核心7个测试二进制首轮通过，新增Job/child/timer/取消/失败批5项首轮通过；当前read-only原因修复另补Linux dw_results通过。原始s4-linux-*日志保留。真实模型方案及候选单轮/全新账本/36请求硬上限已完整本地提交14e9aba，9项离线预算/seed检查通过；新增US$1申请尚待用户明确答复，付费调用0，不复用历史额度。
- 下文S3及更早记录按时间保留；其中“未提交”或“待补”的描述为当时快照。当前继续完整AC、固定样本性能、恢复/生命周期、候选和用户验收，不在此停工等待逐包授权。无远端push/合并。

- S3当前Windows交接/结果功能链已通过，契约RESULTS.md；基线190b48d，最终源码摘要s3-source-production.json。首轮L2 7通过/4失败，失败4项修复后窄复测通过（s3-browser-narrow-second），未重跑原7项。新同步选择回写读composerInput computed形成依赖环，CDP栈保留s3-draft-cdp-stack.log，改用控制器currentSelections；格式化后的dblclick非法表达式已提取reset。app类型/窄能力拒绝/最终lint/生产构建exit0，全部原始失败保留。
- native首轮仅夹具private finish_source编译失败，改用现有TaskHandle.finish后dw_results2、dw_delivery15、nx_launch_live4通过；完整clippy/Task Stop unit首次因缺导入失败，补齐后s3-clippy-fixture-retest exit0，仅既有27 lib及1旧测试告警；task-stop-unit-retest1通过，证明持有准入时Stop可完成且不误停同session其他task。
- L3首轮完整故事通过（s3-live-browser-first）：生产新建Task的可靠Send→精确蓝图版本/交接/只保留结果确认→一次真实解释器执行→Task和独立run共享结果→带入当前草稿零调用→显式Send重新核验结果来源→刷新不重跑且长上下文折叠。当前入口http://127.0.0.1:19089，launcher exec session34278，daemon44340/web33836；实际数据daily-workflow-evidence/20261022/s3-live-first/live-r0ImG1/workspace，ready同父目录；冻结bin s3-runtime-20261023-r1/bin，Web s3-production-first。s3-production-visual.png已只读检查，pageerrors=[]。旧19088/DW-04仍保留；仅19087首轮失败实例已定向停止，数据/trace未删除。
- 后续仍需无终态source的异常新计划入口、完整两故事/事件源/生命周期、固定样本性能与平台迁移、候选模型授权/用户体验/持续观察。DW-06/07与全部AC仍不关闭；当前原生与本地HTTP provider证据不能替代L4/L5。继续推进，不逐包等待授权；无付费调用、push或合并。

- 当前未提交S3工作树基线190b48d，请保留全部差异。新增宿主WorkflowFacts原因码/允许动作、只读结果引用及发送/Queue/Steer再验证、共享WorkflowResult与带来源草稿芯片、历史长上下文折叠；Task Stop不等准入且按task绑定当前turn，新Task复用主聊天可靠提交。Reviews及Usage固定业务轮询已改共享事件刷新。19088仍冻结DW-04，尚不是S3生产版。
- S3首轮app类型/lint通过（早期WorkflowResult thread.id改sessionId，s3-app-structural-first保留）。native首轮仅夹具调用private finish_source编译失败，改用现有TaskHandle.finish后实际dw_results2、dw_delivery及nx_launch_live4通过；日志s3-batch-native-{first,fixture-fix}。完整消费者clippy/Task Stop unit首轮因新unit缺Uuid/ResourceRequest导入未通过，已补导入，尚待复测。
- L2首轮11中7通过/4失败，s3-batch-browser-first与trace保留。新结果三例在同步设置chat.draft时卡住，CDP栈定位到Vue checkDirty/composerInput环；已避免在同步选择写回watch内读取composerInput computed，改用控制器currentSelections。其余Usage失败由oxfmt把已有多语句dblclick展开成非法Vue表达式，已提取reset函数。当前仅复测失败4例（s3-browser-narrow-second，运行结果不得猜测）；过关7例未重跑。额外诊断s3-browser-diagnostic*及s3-draft-cdp-stack.log保留；诊断临时日志/拦截器已移除，诊断浏览器和19084自建Vite已定向停止。后续按实测继续修复并完成S3生产链、S4完整矩阵，不停工、无付费调用、无远端操作。

- S2/DW-04当前Windows文件场景已完成：FILES.md、schema增量/消费者、统一版本选择与冲突流程。首轮native dw_files3及蓝图历史/文件意图/恢复通过；上下文顺序unit1通过；L2新文件与受影响入口22首轮通过。workspace/all-targets clippy exit0，仅既有27 lib及1旧测试告警，app/node类型通过。夹具no-shadow已清理，最终窄lint无告警。
- 保留真实首轮失败：dw04-live-browser-first暴露protobuf JS string解码吞UTF-8 BOM；ReadFileResponse新增可选content_utf8，网关fatal/ignoreBOM解码。仅受影响RPC1和生产文件故事复测通过（dw04-bom-rpc-narrow、dw04-live-browser-bom），首轮trace/截图未覆盖；此前已过L2不重跑。控制器首轮13通过/新增1项仅断言错误文案失配，窄修后通过；dw04-controller-{first,narrow}。全部范围不累加为AC。
- 当前可操作文件运行版 http://127.0.0.1:19088，launcher exec session25992，daemon32804/web56940；数据daily-workflow-evidence/20261022/dw04-live-bom/live-dbyo92/workspace，ready同父目录。冻结bin dw04-runtime-20261023-r2/bin，Web dw04-production-bom，清单manifest/source-bom记录内容身份。原19086保持DW-03；19087保留首轮失败现场。文件故事实际2次本地provider请求，固定旧引用与当前ReadFile确证不同；无付费模型调用。只读截图dw04-production-visual.png已检查，原IDE保留，历史引用长正文仍展开，DW-06必须收进共享可读详情。最终用户体验/全AC未代签。下一继续S3/DW-05/06/07，按同场景收齐后集中检查，不停工等待授权。

- 当前推进S2/DW-04，基线9212d6a上的工作树须保留。FILES.md说明文件引用/冲突/回退实际契约；ReadFile增量及TS生成物/消费者配套。首轮文件RPC3、受影响蓝图历史/文件意图/恢复、文件选择顺序单元及L2文件协作22均通过；app/node类型通过，workspace/all-targets clippy exit0仅既有27 lib及1旧测试告警。日志dw04-batch-*。L2未受影响的已过输入/控制场景不重复跑；测试新增no-shadow告警仅夹具作用域，待清理。当前生产构建已完成，正在构建原生bin，随后同一文件故事验证真实链路。功能尚未提交、DW-04/跨包AC未关闭。既有19086仍是已冻结DW-03实例。

- 2026-10-23 DW-03收尾，源码基线3119cb0加完整差异；摘要dw03-close-batch-source.json。复用现有窄控制RPC/bus，新增固定ID/目标/revision、持久Prepare/Claim/Accepted/Unknown/Retired，Urgent正文原子接收及消费节点checkpoint提交后Applied；Web/CLI/TS协议生成物与旧消费者同包。WORKFLOW_RECEIPTS.md定义容量/兼容/Stop磁盘失败/不重放边界；DW-03本包已定向及生产链验证，跨包AC最终验收仍保留。
- 集中首轮通过：Windows存储/RPC7、bus7、受影响delivery15、客户端15、L2目标/前台/回执9；workspace/all-targets clippy exit0，仅既有27 lib及1旧diagnostics测试告警，app类型/lint exit0。日志dw03-close-batch-*。早期dw03-close-native-check-first.log捕获旧proto构造缺receipt_json，所有消费者已配套；首轮app检查的UUID默认参数类型已明确string，原输出保留于工具记录，后续dw03-close-app-types-second.log及集中类型exit0。源码未变的已过场景没有重跑。
- 首轮L3两条通过/exit0（dw03-close-live-first.log）：普通聊天实际ReadFile/两轮provider/刷新恢复；监督普通消息/临时原文/节点提交消费/Pause/Resume/重开回执/固定目标Stop。provider实际6次本地请求，workflowInputRequests=1、supervisionRequests=1；不计付费模型。生产构建exit0；dw03-close-result.json记录退出码、身份和proof。
- 当前可操作生产入口 http://127.0.0.1:19086，原生进程daemon37644/web15976，launcher exec session13774；实际数据 daily-workflow-evidence/20261022/dw03-live-first/live-GUIOK1/workspace，准备文件同父目录ready.json。二进制冻结在dw03-runtime-20261023-r1/bin，manifest.json记录SHA256；Web冻结产物dw03-production-current，index明确grpc，启动时复制到独立实例web目录。该实例仅指向本地合成provider，不读取用户凭证；旧R8和已批准预览产物未覆盖，旧预览端口不凭历史记录宣称仍在运行。
- 用户再次授权继续全部剩余任务。推进复盘 PROGRESS_REVIEW_20261023.md，阶段/多任务分组 EXECUTION_STAGES.md，新提示词 CONTINUE_PROMPT.md（根NEXT_CHAT_PROMPT已同步，旧提示词保存在planning/2026-10-23）。下一推进S1剩余生产恢复交付及S2/DW-04文件协作，先收齐场景实现与夹具再集中检查，无付费调用、push或合并。

- 最新完整功能（基线9273f92，摘要approval-source-before-commit.json）：具体审批决定的Claim/Accepted/Unknown/Retired回执、固定目标/内容、旧ID关闭和现有broker去重；ApprovalDelivery增量RPC/代理/TS生成物及Web/CLI手工/自动/TUI配套。普通聊天失败不再提前清审批，Tasks审批和直接Stop绑定原turn，不换当前会话。契约和边界见APPROVAL_DECISIONS.md；Stop/Pause/Resume durable回执、跨重启未决清单/分类入口及蓝图临时输入继续后续DW。
- 首轮集中：Windows存储5、实际审批RPC7、grant存储3、原approval_scopes21、客户端8通过；workspace/all-targets clippy exit0，新增4个可合并if告警已修正，最终daemon lib clippy exit0仅既有27告警。app/node类型和lint exit0，变量遮蔽2告警改名后最终app/lint exit0。L2首轮16中14通过/2失败（新增审批7和前台6均通过），另6项监督/审批受影响检查通过；两个旧续聊夹具未等恢复就发送、误把所有资源RPC计为续聊，修复后仅失败2项窄复测通过。日志approval-batch-*及approval-narrow-*保留，未重跑已过场景。
- 首轮R-batch真实RPC/本地provider失败暴露checkpoint列表误读dw回执，实际也影响dw草稿/聊天投递；补命名空间过滤及实际三类存储共存/UUID边界/损坏checkpoint检查1通过，仅失败R-batch窄复测1通过。首轮source摘要和trace保留；native窄复测source摘要另存。正式前端构建exit0，产物approval-production-final显式grpc，未更新运行版/不计L3。完整AC仍未关闭，模型费用0，无push/合并。

- 最新完整功能（基线773810b，摘要view-source-before-commit.json）：宿主位置副本接入标签/选中会话/分栏/文本选区滚动/画布视口；latest_view一次有界读取，窗口独立副本，sessionStorage只存ACK ID。深链优先、缺失源不偷换最新会话，删除失败保持原会话，关联草稿先备份并保留孤立恢复。位置记录不恢复正文/模型/权限，见VIEW_STATE.md。
- 集中检查：Windows存储11、RPC4、客户端34通过（view-batch-{ledger,rpc,controllers}.log）；workspace/all-targets clippy exit0，仅既有27 lib与1旧测试警告。app/node类型exit0；lint首轮控制字符正则/命名遮蔽已修正，最终exit0。L2首轮60中52通过/8失败，view-batch-source.json和全部截图/trace保留；旧计时夹具缺session回调6处、旧非UUID目标1处、新测试inactive-tab误计数1处。修复并补恢复读取不可用守卫后，仅失败/受影响9项窄复测通过，未重跑已通过52项；位置控制器仅窄复测7项通过（view-narrow-*.log）。
- 正式前端构建exit0（view-production-final-build.log），HTML显式grpc网关；原IDE标签/分栏/选区截图已检查，没有更新运行版、不计L3。DW-02仍有生产链/跨平台/迁移/完整验收，AC-01～30不关闭。下一继续控制/审批及其余DW；模型费用0，无push/合并。

- 最新完整功能（源码基线 e4f2a69，摘要 supervision-input-source-before-commit.json）：监督正常消息的正文/身份原子接收及只读/关闭回执、owner/run/conversation固定、旧部分写入不重放；ConciergeDelivery新增RPC/代理/TS生成物配套。监督受控草稿/独立原稿/原ID恢复与低密度恢复栏已接入，单独保存临时蓝图插入原稿；该临时通道仍没有durable消费回执，后续继续补齐。监督面板1s轮询已移除，复用workspace事件/显式切tab刷新；Reviews等其余DW-07入口还未改。契约与实际边界见 SUPERVISION_INPUTS.md。
- 集中证据：Windows存储6、RPC7、受影响concierge集成6及本地provider设置1、客户端8通过（supervision-input-batch-{ledger,rpc,affected,controllers}.log）；workspace/all-targets clippy exit0，仅既有27 lib和1旧diagnostics测试警告。app/node类型、定向lint exit0；最终窄修后app/lint再查exit0（supervision-input-final-*.log）。正式前端构建exit0（supervision-input-production-final-build.log），产物supervision-input-production-final显式grpc网关，未替换运行入口、不计L3。
- L2集中25项首轮23通过/2失败，保留supervision-input-batch-browser原始截图/trace；旧夹具在首次60ms状态读取前锁住历史，改为等待实际输入可用。慢历史场景同时验证下一草稿/发送按钮不被历史查询阻塞；仅复测失败/受影响2项，全部通过（supervision-input-narrow-browser.log），未重跑已通过23项。早期缺fixture业务通知的首轮/第二轮失败亦完整保留，补通知而未加回轮询。监督输入密度截图已检查，保持IDE几何/13px状态，未代签用户最终体验。各轮不累加为AC，模型费用0，无push/合并。

- 最新完整功能：主聊天 Queue/Steer 持久输入，源码基线5e7b81d；提交前摘要 live-input-source-before-commit.json，完整提交以Git对照。契约与容量/兼容/边界见LIVE_INPUTS.md。复用原bus和根ReAct，无新runner；正文与回执sync WAL后ACK，根独占claim，保存上下文后Applied，结束未取Unconsumed，崩溃/保存未知不重放。Web/CLI能力检查、原稿恢复、固定模式/turn与简短状态配套；监督和控制/审批durable回执仍待实施。
- 检查均exit0：Windows存储14（live-input-ledger-first.log）、bus6（live-input-bus-second.log）、RPC最终15（live-input-rpc-final.log，含无owner对账/源删除/重复/确切原文上下文）；客户端32（live-input-controllers-third.log）；L2最终3（live-input-browser-final.log，实际sendChat回调的assistant活动不会冒充消费），受影响聊天/IME/分离/网关23（live-input-affected-first.log）。app/node类型和定向lint exit0（live-input-final-*.log）；全workspace/all-targets clippy最终exit0，仅既有27 lib及1旧diagnostics测试警告（live-input-workspace-clippy-final.log）。正式前端构建exit0（live-input-production-final-build.log），产物live-input-production-final，未替换运行中的生产入口、不计L3。所有轮次重叠不加总为AC，模型调用0，无push/合并。
- 首轮失败保留：host check三处Rust类型错误修复后通过；新增控制器测试误用restore方法，改loadAsNewCopy后通过；L2切换连接导致配置重载，按键时ready=false，诊断trace保留在live-input-browser-debug*，补充ready前置后通过，未放松产品守卫；初轮clippy新增bus嵌套if警告已修正。无额外产品决策或IDE布局变更。

- 最新完整功能候选（基线2a536f8）：固定live-control目标，见LIVE_TARGETS.md。InterruptRequest新增target_kind/session_id/turn_id/run_id，AbortChatRequest新增session_id/turn_id；无目标/过期/串目标/停止中拒绝，蓝图Pause/Resume/Cancel不接受空run_id。CLI通过不等admission的control_target取得所有者，Web在await前快照目标；旧host能力不足时明确失败。TS生成物、真实/Mock网关、实际调用方和必要检查同功能配套。Queue/Steer仍无durable接收/消费回执，审批失败仍待修，不能关闭DW-03。
- 此功能检查：全workspace/all-targets check首轮两个旧smoke构造缺新字段失败，更新消费者后exit0（targets-workspace-check-first、targets-workspace-check-consumers-retest.log）；真实RPC最终11通过/exit0（targets-rpc-final.log），包括旧turn不能干扰同session新turn、错误chat通道不入蓝图、Stop后拒绝输入及控制查询不等admission。全workspace clippy首轮exit0，修正foreground测试模块位置新增警告后最终exit0，仅既有27项lib及1项旧diagnostics测试警告（targets-workspace-clippy-final.log）。
- 原生聊天烟测8通过、approval scopes RPC3通过、R-batch本地HTTP/审批/版本/插件1通过；R-batch改用实际CLI pause再复测1通过，蓝图精确pause/resume/cancel烟测1通过，均exit0（targets-smoke-chat-first、targets-approval-scopes-first、targets-r-batch-first、targets-r-batch-cli-retest、targets-smoke-blueprint-first.log）。L2网关/原聊天10通过，增加目标对象在await期间修改的窄复测1通过；受影响聊天/监督26通过。各轮重叠，不加总作AC。app类型/定向lint、正式前端构建exit0（targets-final-app-types、targets-final-lint、targets-production-final-build.log）。未更新生产运行版、不计L3，模型调用0，无push或合并。

- 最新增量为主聊天/监督分离与宿主前台占用快照，基线67e0578。主聊天不再按execution.active替换；状态/监督入口/直接Pause与Stop保持原IDE几何，Stop可越过在途Pause。实际契约与剩余旧Interrupt边界见FOREGROUND.md。新增只读检查不准备资源schema、不写checkpoint或业务通知；未知schema保守不可用。
- 本功能证据：RPC首轮8通过/1失败，取消蓝图按真实语义返回Aborted，测试错误地unwrap；修正为断言Aborted并确认槽位移除后9通过（foreground-rpc-first、foreground-rpc-cancel-retest.log）。追加只读构造后真实DB测试1通过（foreground-read-host-first.log），RPC9复测通过/exit0（foreground-rpc-read-retest.log）。daemon首次及只读构造最终clippy均仅既有27项lib警告/exit0（foreground-clippy-first、foreground-clippy-read-final.log）。app/node类型与定向lint exit0（foreground-final-*.log），lint首轮两处测试参数shadow已改。L2初始13通过；菜单/rewind复测15通过；Stop优先级5通过；监督/提案/Execution受影响27通过；A/B追加6通过。各轮有重叠，不相加作AC通过。所有原始日志foreground-*.log保留。
- 隔离预览适配构建、正式前端构建均exit0；foreground-isolated-build.log、foreground-production-final-build.log及对应目录。保留19082/19085冻结预览，没有将新产物冒充运行中的生产daemon。新Main截图在foreground-browser-ab-first内，已检查文字和布局，用户最终体验未代签。

- 最新完整功能：主聊天/新聊天的原始草稿、选择及 pending ID 接入已完成，源码候选 f60b614 加本功能差异；提交前摘要 composer-input-source-before-commit.json。同步主界面恢复栏/共享 Composer、source_available RPC、预览适配器、必要检查；完整提交后以 Git HEAD 对照摘要。本功能不关闭整个 DW-02/03。
- 当前检查：Windows RPC7/exit0（composer-source-rpc-first.log）；daemon clippy仅既有27项警告/exit0（composer-source-clippy-first.log）；客户端27/exit0（composer-controller-final.log），app/node类型及定向lint均exit0（composer-final-*.log）；L2聊天9/exit0（composer-browser-final.log），共享Composer/rewind/sequence/SSE/文档恢复18/exit0（composer-affected-first.log）。隔离预览适配构建及正式前端构建exit0（composer-isolated-build-first、composer-production-final-build.log）；尚未更新真实运行版，不算L3。
- 首轮失败保留：整合类型3处（未用ref/类型导出/异步窄化）修正后通过；lint未使用响应式版本读取改为响应式Map，测试参数重名已改；首轮L2的配置尚未ready，补测试前置等待；第二轮仅旧`.chat-thread`选择器错误，改实际`.chat-thread-inner`。没有改生产模型准入来满足夹具。早期6/8项与最终9项有重叠，不累加为AC。
- 主聊天当前可按原ID对账、保留较新输入并手工恢复宿主确认副本；新文字13～14px，技术信息在详情。余下监督持久输入、标签/位置、Queue/Steer固定目标、控制/审批、主监督分离及DW-04～09继续推进。最终用户体验验收、付费模型/数据授权、实际持续观察和Linux仍待完成，无付费调用、push或合并。

- DW-00 基线完成，提交 1b39ce8；G01～19、接口/容量/性能判定及最小复现见 BASELINE.md。复现发送拒绝清空、IME synthetic Enter 提交、rollback 失败不呈现、蓝图替换主聊天。简单切页原有 KeepAlive 保留输入，没有伪造该项失败。
- DW-01：r1 67f56d9 未批准。r2 5e40cbf 复用原 App/IDE，三组已获用户原文“批准三组 r2，保留现有 IDE 布局”。具体范围见 PREVIEWS.md。最终产品视觉签字尚未取得。
- DW-02 宿主/文档恢复已提交97436a6，主聊天接入见上述最新功能；监督/位置待接入。r3低密度隔离修订a8653cf，19085，原r2保留19082；r3未代填用户签字。手工发送协议9aa23cc，未决身份启动前对账f60b614均已完整本地提交。DW-02～09和AC-01～30均未关闭。
- 最新体验约束：“界面不要太多小字,信息量不要太大,要兼顾使用体验”。保留 IDE 几何；新增主要文字/操作约 13～14px，默认状态与必要操作，身份、revision、窗口和长诊断收进详情。已用于文档恢复栏和恢复弹窗，后续继续遵循。

## 当前功能与限制

以下保留9aa23cc/f60b614协议阶段的详细记录，其中尚未接入Composer描述的是当时边界；当前已接入范围以上述最新功能为准。

新增 pending_reconciliation 能力和 reconcile_pending：仅当前宿主 owner/project 的 chat/new_chat 未决身份先对账；未接收身份持久关闭、已接收保持原 turn，监督身份不混用，原始副本不改写。客户端在新发送前检查能力并对账，失败即保留输入且不启动新回合。基线9aa23cc；Windows真实RPC6、客户端9、SSE3、app类型及daemon clippy通过，仅既有27项lib警告；pending-delivery-*.log。此增量没有更新生产运行包，未接入Composer，不计AC关闭。


DW-03 接收协议完整提交 9aa23cc：增量 ChatDelivery RPC；SendChat.options.delivery 为可选精确 request ID/expected revision/新会话 UUID。普通旧请求保持原语义；新客户端先探测能力，不在旧服务静默回退受保护语义。接收把用户输入/session revision/receipt/关联新 task expected revision 一次 sync WAL 提交，再将 Started 持久化后进入原 runner。重复或重开只查询同一 ID，不新建执行；回执与模型结果分开。普通断流仍取消，获批 detached 沿用原 policy。

缺失查询不是明确未接收：同一 admission/resource gate 下 retire 未接收 ID；晚到发送拒绝，若接收已胜出则返回原回执。receipt 8192/每 scope16 MiB/单4 KiB，永久有界凭据不静默淘汰；达到上限拒绝新输入。第一次实际 UI 接入仍待完成，不能声称 Composer 已不丢输入。

本功能 Windows 检查：源码基线 a8653cf 加本功能完整差异；全 workspace/all-targets cargo check exit0（delivery-check-first.log）；存储首轮6通过，新增 retire 后8通过（delivery-host-first、delivery-host-retirement-retest.log）；RPC 首轮2通过/1失败，continuation样本10000 token低于真实根预算预留，合成预算改100000且max_tokens128后5通过（delivery-rpc-first、delivery-rpc-retirement-retest.log），未改预算守卫。客户端7通过（delivery-client-first.log），app类型exit0；SSE fixture和chat-sequence 3通过/exit0（delivery-browser-transport-first.log）；全workspace clippy exit0（delivery-clippy-first.log），发现一处新增可合并if后修正，窄复测仅既有27项lib警告/exit0（delivery-clippy-retest.log）。显式移除模型的宿主拒绝补强后 RPC5通过及窄clippy仅既有27警告/exit0（delivery-rpc-model-retest、delivery-clippy-model-retest.log）。未报告用量改为null（真实0仍为0），状态栏不显示虚构token数；新增流提前结束保留执行未知。定向计时/序列/回执5通过（delivery-usage-browser.log），终态三分支3通过（delivery-terminal-browser.log），有重叠，不累加作AC。app/node类型、定向lint通过，生产前端最终构建记录 manual-delivery-protocol-final-build.log。协议/客户端/检查同完整功能提交；模型费用0。

宿主 storage/recovery.rs 在 EXECUTION_STATE 使用独立 namespace，复用 durable sync 批写及 resource_gate；不进入资源历史、checkpoint 或业务通知。每记录 1 MiB、每 owner/scope 64 MiB、活动 512/含 tombstone 总 4096、metadata 分页 50。超过限制如实失败，无静默淘汰。支持 CAS、准确 request 重试、discard tombstone、rename 转移、损坏记录隔离及原始导出。增量 Recovery RPC 的生成物、Web 代理与测试实现同功能更新；旧服务明确 Unimplemented。

前端按窗口/实体隔离，750ms debounce/5s 最长发起，至多一个在途备份。未知重试原请求；旧 ACK 不清新输入。保存按对应内容清理，明确丢弃需宿主确认，丢弃回执晚到期间的新输入另存副本。文本/画布无效原始稿与 Prompt 草稿均保存原始输入；恢复先比较当前文件，只放进编辑器，源副本保留，不自动覆盖磁盘。无持久浏览器正文缓存。

限制：接口按 Recovery RPC ACL 和宿主 owner/scope 隔离；恢复输入不能赋予原模型/资源/工具权限，使用时仍需当前专用入口校验。每次备份在有界 scope 内扫描配额，最坏开销还需 DW-07 测量。位置、选项、未决发送身份尚无完整端到端接入。异步恢复/关闭新增 revision 守卫后，恢复与 workspace-reliability 共 13 项复测通过/exit 0，recovery-browser-final.log。本功能定向证据不能代替 L3 生产链、Linux、真实模型与最终体验验收。

## 检查与证据

证据根 F:/space/06_Projects/Agent/daily-workflow-evidence/20261022。源码起点 5e40cbf 加此完整功能差异；提交前保存源码摘要。Windows x64；没有付费 provider。

| 检查 | 实际结果与原始证据 |
|---|---|
| run_native.py cargo check --locked -p metteur-daemon --tests | exit 0；冷编译 32m25s，工具会话输出保留 |
| cargo test --locked -p metteur-daemon --lib storage::recovery::tests | 首轮 9 passed/exit 0，recovery-host-first.log；追加 rename 容量边界后 10 passed/exit 0，recovery-host-capacity-retest.log |
| cargo test --locked -p metteur-daemon --test dw_recovery | 首轮 3 passed/exit 0，recovery-rpc-first.log；Box 解析调整后 3 passed/exit 0，recovery-rpc-box-retest.log |
| cargo clippy --locked --workspace --all-targets | exit 0，recovery-protocol-clippy-first.log；发现新的大 enum 警告，改 Box 后 cargo clippy --locked -p metteur-daemon --lib exit 0，仅既有 27 项警告，recovery-clippy-box-retest.log |
| node --experimental-transform-types --test scripts/test-recovery.mjs | 原 6 passed；追加丢弃晚到/未知后新输入 2 项，8 passed/exit 0，recovery-client-discard-race.log |
| vue-tsc app、tsc node、定向 oxlint | exit 0；追加控制器竞态后 app 类型亦通过；定向 oxfmt 已整理 |
| Playwright workspace-reliability、blueprint-documents、file-write-boundaries | mock transport/0 retries/1 worker，16 passed/exit 0，editor-browser-retest.log |
| Playwright daily-recovery.spec.ts | L2 transport fixture/0 retries/1 worker，3 passed/exit 0，recovery-browser-third-retest.log；刷新恢复、两个窗口/丢弃、容量拒绝，不能证明 DB 耐久性 |
| vite build --outDir <证据根>/document-recovery-production | exit 0，document-recovery-production-build.log；已有 Monaco chunk/插件耗时提示；控制器和异步 UI 守卫修复后最终构建 exit 0，document-recovery-production-final-build.log，产物 document-recovery-production-final |

首轮失败均保留：旧文档 create-only 恢复被新增冲突守卫误拦，已修正；恢复 fixture connected=false 和 Monaco 自动配对改变输入分别修正测试前提；实际 Vue proxy structuredClone 抛错已改 JSON 数据克隆，保持 raw 原始字符串。r2 缺原 Tailwind source 的几何错误已在批准前修正并复查截图，原失败图保留。

## 运行和下一步

- 1801 旧复核服务实际未运行，R8 文件/DB 保留；5317～5319 属 Windows 排除端口，不改系统配置。
- 19080 开发 mock；19081 r1 静态隔离预览；19082 为已批准 r2 冻结静态产物。归属见 PREVIEWS.md。当前生产构建目录不是运行中的真实 daemon 实例。
- 收齐此功能定向复测，保存源码/产物摘要和完整本地功能提交；继续 DW-02 聊天/监督/位置及 DW-03 持久接收事务，然后依赖持续推进余下工作包。
- 可复核问题：namespace/配额、未知回执边界、未决输入接入现有 runner 的最小事务；三项产品决策不重新询问。

推进/推送原则：一个独立可验收功能一个完整本地提交，必要协议/客户端同属功能。保留历史，fork 顺序快进，不强推、不推 origin、不自动合并。新推送/发布依当前明确授权。本阶段持续推进，不逐包停等；具体视觉版本、付费模型数据/额度依实际门槛取得输入。无远端变更、模型调用 0。
