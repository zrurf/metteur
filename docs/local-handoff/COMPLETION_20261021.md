> 2026-10-21 用户验收启动复核：旧 2ada184 包的便携启动存在缺依赖与过早 ready 的缺陷，原 portable-demo-smoke-uu8r3mis 结论撤回。已由现有锁定依赖生成独立 Node bundle，并用无源码/node_modules 的目录完成实际蓝图读取、文件工具两轮调用和会话历史验证。新证据：portable-bundled-runtime-rpc-proof-bz4l8brs / portable-demo-smoke-ajwsusqm。原包保持不变，新包另存；付费评测及生产二进制未改变。分批发布准备见 PUBLISH_BATCHES_20261021.md。

# Metteur 本地收尾验收 · 2026-10-21

已补齐本轮授权范围内的剩余工程实现、质量修复、真实 DeepSeek 验收和本地交付准备。45 个 NX 任务及 SPEC §8.3 的 88 个场景全部更新到 `LOCAL_ACCEPTANCE_MATRIX.json`。最终视觉定稿保留为用户事项，未以技术测试或模型意见代替。

## 本轮修复

- 接入受操作员配置约束的 Addon 模型意见 worker：绝对路径、隔离进程、有界来源文本、版本/digest/provenance 校验；既无通用工具，也不能产生 human acceptance 或 permission grant。检查前后复核 lease/revision，变更、禁用后的旧结果不再接受。
- 修复全局 tooltip 在悬停时移除 iframe title 的问题，保留嵌入页面可访问名称；加入真实浏览器回归。
- 修复 Windows oversight 测试把活动 RocksDB LOCK 放入源快照目录的夹具布局，使用实际 `.metteur/db` 目录；没有跳过失败测试或全局忽略 LOCK。
- 补全 18 个冻结种子、真实原生 daemon/Web 配对评测器、跨进程成本预留账本、严格合成命令审批；候选命令适配来自 development 首次失败。
- 补充预算预留进程的创建身份校验：进程异常退出或 PID 复用后，未决预留转 unknown 并阻止继续调用，保留成本；Windows/Linux 各 11 项离线测试通过，无新增付费请求。
- 补充真实新消息替代等待上下文、DB 重开和独立原生进程崩溃/重启测试；未决 Job 标为 indeterminate/需处理，不按旧 PID 错认或自动重放。

## 本地门槛

| 范围 | 结果 | 证据目录（交接包 evidence/ 下） |
|---|---|---|
| 原生 Windows 全工作区 Rust | 900 报告通过，0 失败/0 忽略；clippy 退出 0 | native-full-rust-layout-fixed-gwgv_txm；native-final-clippy-cd_vskfe |
| Linux 冻结构建、测试、clippy | 915 报告通过，0 失败/0 忽略；三步骤退出 0 | linux-final-frozen/first-gpw6sw0x |
| 有效应用/Node 配置类型、lint、生产构建 | 全部退出 0 | frontend-effective-types-completion-81de1idc；frontend-lint-completion-68_oxuhr；frontend-complete-production-pinned-4bbp3mez |
| 完整前端浏览器回归 | 112 通过，0 重试 | frontend-full-e2e-completion-huiigsyr |
| 双模型开发/留出 | 最新 24/24；冻结回执 48/48，原判及更正分列 | completion-model-report-20261021.json；held-out-offline-regrade-20261021.json |
| 真实双模型资源完整主链 | 每模型 7 项检查通过 | full-resource-chain-5phc_4je |
| 真实双模型客服/监督 | tool-free 客服、请求回执、独立提案拒绝后无动作通过 | actual-model-roles-t9ug9vxd |
| 签名 Addon 安装页闭环 | 4 项通过，0 pageerror；真实模型意见与人工测试验收分离 | installed-addon-browser-LE6lCJ |
| 当前生产页面/排序 | 7 页面 × 2 主题 × 2 宽度，共 28 次通过，0 页面/控制台错误 | native-surface-browser-qEhSvO |
| 原生等待 Job 强制崩溃与重开 | 同 task/session、无新模型调用、无重放通过 | actual-pending-job-restart-xbuq51_e |
| 无依赖的交付启动器 | 复制后的 Windows 二进制+生产 Web+Node 24 启动；模型调用 0 | portable-demo-smoke-ajwsusqm |

测试数字为程序报告执行次数（含可能的辅助入口），不是需求数。Linux 为本机 WSL，Windows 结果来自 MSVC 原生构建，二者分别记录。原始首次失败、真实退出码及修复复核均保留。

## 模型与费用

Flash 与 Pro 真实调用均已完成。全部 379 次生成的峰值费率保守估算为 **US$1.82139858**，未超 US$5；实际账单未知。数据全部为隔离合成内容，原凭据未复制。详见 `../nx-m4/EVALUATION_20261021.md`，没有“新 Prompt 显著更优”或“缓存一定命中”的结论。

## 交付与使用

新版本包包含 source、完整 history/metteur.bundle、已验证 Windows 二进制、生产前端、依赖锁、日志、首次失败、截图、原计划与摘要清单。`verify_package.py` 只读校验全部列出文件；bundle 已在新目录克隆、核对 clean HEAD 并运行 git fsck。最终 SHA 与恢复回执在包外同名 `.sha256.json`，包内 STATE.json 记录精确提交。旧 cedfb59 候选包和原云端压缩包保留。

安装 Node 24 后在包根运行 `powershell -File .\Start-demo.ps1`，按输出的 URL/workspace 打开免费的合成演示，无需重新编译或 node_modules。Ctrl+C 停止。实际 DeepSeek 的配置与预算说明见 `LOCAL_SETUP.md`；普通启动不会自动载入 D 盘旧配置。不要将本轮合成测试授权理解为任意真实业务数据的付费授权。

## 验收边界

- 已完成工程复核；`页面复核.html` 和截图供用户确认最终视觉。此前 R1 接入许可保留，但尚无用户对这些最终页面逐页定稿的记录，未虚构 APPROVED。
- 保留既有 clippy/格式与前端 chunk 警告；当前门槛通过，不声称无警告。1k 节点/10k 合成调用、12 次 DB 重开与重复事件压力已有有界证据；未执行多日 soak、任意旧二进制互写或所有浏览器/操作系统组合。
- 原 M1 本地仓库未改动，当前工作分支为 `codex/local-acceptance-20261020`。这是本地交付，未 push、更新远端 PR、合并或发布；远端发布未在本轮请求中执行。
