# 最终验收与分阶段推送

最新收口：用户要求完成原计划全部遗留并验收后推送；正式19103已升级、原数据核对完成，当前源码/运行及完整范围索引见[FINAL_CLOSURE.md](FINAL_CLOSURE.md)。下方旧19103、未升级、仅讨论及旧HEAD描述均为历史快照，以最新收口记录和根CURRENT.txt为准。

2026-10-23（Asia/Shanghai）。本轮按用户“最后验收然后推送上去”的指示交付。产品为宿主`ccecd9c`、已批准前端`65cc92f`；后续提交仅补验收工具、证据和交接文档。完整范围见[AC矩阵](ACCEPTANCE.md)，保留各功能独立提交。

## 验收结论

本轮28项通过、2项暂接受后续人工补验，没有等待中的本轮门槛。

- AC-01～18、20～22、24～26、29～30：26项已有技术证据，复用未受影响的宿主、组件、真实生产故事、Windows/Linux、生命周期、升级和冷恢复结果，未重复执行。
- AC-19/28：原观察实际21,784,891ms（约6小时3分）、353样本，无已记录错误或长采样间隙。原进程退出，原8h目标没有跑满；退出原因未证实。已向用户说明后，用户明确“记作通过,时间已经够了”，据此通过（用户接受现有时长），不重启或延长测试。
- AC-23/27：用户明确同意暂接受、后续人工补验，不阻塞交付。真实Windows IME和实际S3睡眠尚无操作证据；[清单和样本](MANUAL_ACCEPTANCE.md)保留，后续问题按具体场景修复。

持续记录中的恢复元数据恒定4行，最大采样间隙62,635ms。两个post-GC JS堆峰值18,803,356／18,815,368 bytes，daemon／Web private峰值84,434,944／7,487,488 bytes，均在原冻结守卫内。原19093 Web身份保留，不能改称新19103进程已经长测。持续原件、最终汇总与用户接受决定分别在`s4-observation-ccecd9c`、`s4-final-review-20261023/continuous-result.json`及`user-duration-acceptance.json`。

三项获批真实模型任务均首轮通过，实际14/36请求；峰值费率估算US$0.0298446，provider实际扣费金额未知。本轮没有新增模型调用，也未重跑故事、性能基线或已通过的故障场景。

## 最后交付核对

源码与冻结产物再次逐文件比对：195项前端源码、361项原生源码、270项前端资产、2个原生二进制均零差异。最终Git差异检查发现并删除DESIGN.md的一行多余文末空行；没有产品实现变化。新增文件的凭据形状检查没有发现长sk-/ghp_/github_pat_字面量，未打印凭据内容。

正式入口仍为http://127.0.0.1:19103/。最后核对时旧运行服务均已退出，已使用同一冻结程序、同一已批准静态前端、原fixture配置和原数据库目录恢复；没有重新seed样本、重写原文件或自动打开工作区。实际HTTP内容等于冻结index，真实grpc-web→daemon的ListWorkspaces读取成功，provider调用0、Run/Send 0。恢复夹具首次误要求独立UI携带旧fixture nonce，已修为比较实际冻结index；原错误单列保存，未修改产品或重跑业务。

当前daemon PID26212、Web PID30108、本地provider PID46196，恢复启动exec88694；历史PID不再视为活跃。运行清单为`s4-final-review-20261023/ready.json`，实际读取证据`availability.json`，原数据仍为`s4-live-lifecycle/live-kH5vsm/data`。服务会随进程退出，后续使用前按实际清单核对。

已知回退边界仍保留：R8不能原地列出新增dw namespace运行数据；安全回退需要升级前冷副本，并保全当前新稿及后续修改。见[RESTORE_GUIDE.md](RESTORE_GUIDE.md)，不称原地降级兼容。

## 推送边界

GitHub连接器本次不可用，远端核对通过本机Git和GitHub CLI完成。已有上游Draft PR [#9](https://github.com/zrurf/metteur/pull/9)仍OPEN，head为用户fork的`codex/nx-final-acceptance`，已发布起点`6b19ffa`；上游main核对为`503299f`。当前实施分支`codex/daily-workflow-20261022`是该head的顺序后继，沿用该PR更新最终范围，不另开重复PR。

只推`https://github.com/shizhende574-creator/metteur.git`，目标`codex/nx-final-acceptance`。每批核对远端ACK后再推下一批，不squash／rebase／强推、不推origin、不自动合并或转ready、不主动触发可选全量CI。沿用已审阅的`scripts/publish-batches.py`冻结清单和执行守卫。

| 新批次 | 范围 | 顺序快进边界 |
|---|---|---|
| 1 | S0预览与S1草稿／投递／监督／控制 | `9212d6a` |
| 2 | S2文件引用／冲突／回退 | `190b48d` |
| 3 | S3共享结果／交接／事件刷新 | `64d464f` |
| 4 | S4真实故事／事件连接／生命周期修复 | `ccecd9c` |
| 5 | 恢复／模型验收／已批准UI | `65cc92f` |
| 6 | 生产UI核验／人工后置／最终验收交付 | 本文所在完整提交，以外置冻结清单SHA为准 |

远端结果是可变事实，本文件在推送前提交，不预写成功。完整批次SHA、实际推送输出、远端最终head、PR更新及本地恢复bundle摘要写入工作区外`s4-final-review-20261023`；根CURRENT.txt记录最终实际回执。若远端变更或ACK不符，脚本会停止，不能越过守卫。

证据根：`F:/space/06_Projects/Agent/daily-workflow-evidence/20261022`。原始失败、批准预览、旧数据与历史回执全部保留。
