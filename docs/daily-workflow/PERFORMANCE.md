# 固定样本性能记录

2026-10-23，Windows当前机器。候选b6a451d的冻结程序/源码摘要在`s4-runtime-b6a451d/manifest.json`；Web为`s4-production-first`。对照使用保留的190b48d生产运行版19088，候选19091。原DW-00 dev/mock记录模式不同，不作为生产速度提升证据。

同一Chromium进程、1360×900、新浏览器context、相同两文件内容；先对照后候选，每版20次热标签切换、30次插入并等待文字可见。阈值沿用BASELINE.md预先约定的max(绝对预算, 对照×1.25)。原始样本、机器/请求/时间/截图见`s4-performance-first/measurements.json`，首轮通过。

| 测量 | 对照 | 候选 | 阈值 |
|---|---:|---:|---:|
| 浏览器新context至文件树可见 | 458ms | 225ms | 5000ms |
| 热切页p95 | 93ms | 79ms | 500ms |
| 插入至可见p95 | 37ms | 40ms | 100ms |
| 编辑后JS used heap快照 | 32.1MB | 33.5MB | 观察值，无先验通过阈值 |

宿主程序预先运行，启动项不是冷daemon启动。单轮顺序测量不能证明统计性提速。浏览器自动化的插入不是实际Windows IME，后者仍需用户确认。候选与对照均无pageerror。

每版Tasks空闲10秒均无QueryContinuations/GetConciergeState/GetOversightReviews的1s/1.5s轮询。可见请求是最后一次延迟恢复备份，以及既有5秒ListWorkspaces基础状态；不把基础连接/工作区读取或本地时钟隐去。另有S3定向重复通知/Usage/Reviews合并证据，未因性能测量重跑该场景。

持续观察将单列实际开始/结束时间、样本次数、恢复记录数量和内存，不将上述短区间称为长期稳定性通过。1000次同稿备份的真实DB恒定行数/无业务通知属于独立存储检查，不等于磁盘物理字节不增长。

最新候选ccecd9c另在s4-performance-ccecd9c定向测量，复用未改动对照原始样本，没有重跑对照：新context到文件树260ms、热切页p95 83ms、插入p95 39ms、编辑后JS heap 30.7MB，原5000/500/100ms阈值均通过，10秒无目标业务轮询。最新8小时双窗口观察为s4-observation-ccecd9c，逐分钟确认两个独立原稿和恒定记录数、每10样本采集post-GC JS heap与native进程内存；64MiB/page及max(256MiB,初始private×1.25)守卫在开始时冻结。采样间隙超过90秒会单列，不能称完整连续窗口；不是实际OS睡眠验收。旧eeb930c单窗观察实际71.2分钟/71次ACK后停止，保留原数据，不能冒充修复候选的8小时。


UI-CONSISTENCY r1批准接入后，正式19103只补当前前端固定样本，基线仍复用s4-performance-first，未重跑对照。s4-performance-ui-r1：新浏览器context到文件树262.13ms、标签p95 91.72ms、键入p95 40.84ms、JS heap 32.21MB；均满足原5s/500ms/100ms阈值。10秒空闲没有相关业务轮询，无page error。不作统计提升结论。原双窗口观察继续验证未改的输入/事件/存储实现并保留旧Web进程身份，不能称新19103进程已运行8h；AC-19/28仍待持续证据齐备。

最终核对（2026-10-23）：原观察进程退出，实际elapsed 21,784,891ms（约6小时3分）、353样本，最大采样间隙62,635ms，无已记录错误或长间隙；恢复元数据恒定4行，两个post-GC JS heap峰值18,803,356／18,815,368 bytes，daemon/Web private峰值84,434,944／7,487,488 bytes，均在原冻结守卫内。汇总器在最终核对时如实标REQUIRES_REVIEW，而非8h完成。告知后用户明确“记作通过,时间已经够了”，据此AC-19/28通过（用户接受现有时长），不延长或重跑。原19093身份保留，新19103不冒称长测；原始样本及用户决定见s4-final-review-20261023。
