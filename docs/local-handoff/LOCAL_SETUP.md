# 本地恢复和环境准备

2026-10-20 更新：当前本机候选及验收状态见 `LOCAL_ACCEPTANCE_20261020.md`。Windows 编译工具已安装并通过实际编译/链接探针；项目级验收仍在进行。

## 一、只读校验后在新目录恢复
在解压包根目录：

```powershell
python .\verify_package.py
Get-Content .\STATE.json
```

`STATE.json.source_head`是权威交付提交。选择一个尚不存在的目录恢复（不要直接复制下面的目录名覆盖已有文件夹）：

```powershell
git clone .\history\metteur.bundle <新的恢复目录>
git -C <新的恢复目录> rev-parse HEAD
git -C <新的恢复目录> status --short
```

对照STATE，要求提交一致、工作区干净。`source/`为完整可读副本；优先用bundle保留历史。原本本地工作区已配置，不另行强制规定磁盘路径；先检查已有目录与改动，再决定如何整合。

克隆后的origin可能是bundle文件，不是GitHub。先 `git remote -v`，只读核对，暂不变更推送配置。若本地有更新，使用共同祖先和diff审阅，不能以交接包覆盖全部本地改动。

## 二、工具链
- Rust1.99.0，遵循rust-toolchain.toml和Cargo.lock。
- Node24.13.1（或经明确验证的兼容24版本），pnpm11.24.0，使用modules/webcore/package.json的packageManager与pnpm-lock.yaml。
- Python3.11/3.12。
- Windows x64 MSVC开发终端、Windows SDK、CMake、protoc、LLVM libclang；`LIBCLANG_PATH`指向实际libclang.dll目录。`where.exe link`应是MSVC linker，避免Git GNU link。
- WSL可供辅助开发，但不能代替原生Windows证据。
- 如缺依赖，列出官方来源及安装项；凭据/网络安全配置按本地授权操作，不以交接包自动授权任意安装。

本机已按用户授权安装 Visual Studio Build Tools 2026（18.11.0）到 `D:/Tocoding/VSBuildTools2026`，包含 x64 MSVC、Windows SDK 10.0.26100.0 和 CMake 4.3.1；LLVM/libclang 23.1.3 位于 `D:/Tocoding/LLVM/bin`。安装均返回 0，无需重启。Rust 1.99.0 链接运行、使用 Windows SDK 的 C++ 编译运行、libclang DLL 加载均已通过。

本机留有独立环境启动器，设置当前子进程的 VS、Rust、libclang 和 D 盘构建缓存，不修改全局 PATH：

```powershell
py -3.12 -X utf8 F:/space/06_Projects/Agent/workspace-review-2026-10-20/run_native.py --probe
py -3.12 -X utf8 F:/space/06_Projects/Agent/workspace-review-2026-10-20/run_native.py cargo test --locked -p metteur-shared --lib -j 1
```

这些路径是本机配置，迁移机器时应重新定位工具。安装日志和校验记录在 `F:/space/06_Projects/Agent/workspace-review-2026-10-20/native-tools-install`。

## 三、最小构建（先核实际环境）
从恢复仓库根目录：

```powershell
cargo build --locked -p metteur-daemon --bin metteurd -p metteur-web-server --bin metteur-web
Set-Location modules/webcore
corepack pnpm install --frozen-lockfile
corepack pnpm typecheck
corepack pnpm build
```

有效应用/config 类型检查已修复并在本机通过，保留严格源码检查。构建若使用CARGO_TARGET_DIR，后续启动必须指向同一目录。安装依赖可能访问官方包注册表，但不应调用模型API。

## 四、本地无付费演示入口
依赖及debug二进制、前端dist准备好后，从仓库根目录运行：

```powershell
node modules/webcore/scripts/run-nx-local.mjs --serve-only
```

它创建独立测试工作区、loopback可控provider和服务，不加载用户实际模型配置。读取控制台/ready.json中实际URL与workspace，不硬编码云端18981端口。

可选环境变量：METTEUR_VALIDATION_DIR、CARGO_TARGET_DIR、METTEUR_DAEMON_BIN、METTEUR_WEB_BIN、METTEUR_WEB_DIST、METTEUR_WEB_PORT。值必须按本机选择，不能复制云端 `/workspace/scratch/...` 路径。

`M3_LOCAL_BLUEPRINT`是夹具识别词，仅在这个隔离provider的新会话中使用，触发一个需人工确认的本地蓝图提案；Decline应结束。不要把它当成真实模型成功。Job样例已改为跨平台 Node 定时命令，其 Windows shell 探针已通过；完整 daemon 流程另行记录。

## 五、原生Windows必要验证
从干净候选仓库根目录，以一个尚不存在的绝对目录：

```powershell
.\scripts\nx-windows-targeted.ps1 -OutputDirectory <绝对的新证据目录>
```

只执行锁、storage、恢复三组，失败即保留结果并修复；不要开启全量矩阵。脚本不自动安装依赖、不调用模型。PowerShell执行策略若阻止脚本，按当前系统/用户策略处理，不擅自全局放宽安全设置。

## 六、可选离线页面检查器
参考 `docs/nx-m6-completion/RENDERER.md`，以宿主受信任配置设置绝对路径：
- METTEUR_PREVIEW_NODE：实际node.exe。
- METTEUR_PREVIEW_RENDERER：本仓库modules/webcore/scripts/preview-inspection/render.mjs。
- METTEUR_PREVIEW_CHROMIUM：已安装Chromium路径；否则需要已安装的Playwright Chromium。

不禁用沙箱，不使用用户浏览器profile，不让插件选择任意命令/URL。缺配置显示not_configured；失败显示failed，不改成假成功。包内样例签名使用公开测试密钥，只适合隔离演示，不是生产信任凭据。

## 七、数据与回退
停止写入后完整备份数据库和工作区，或使用项目checkpoint机制；不要随意复制RocksDB部分文件。保存Prompt源、个人资源、日志、journal、报告与批注。旧二进制不能保证读写新schema；先在副本验证，禁止手改schema标记“兼容”。


## 2026-10-21 已验证的本机快速启动

MSVC/Windows SDK/CMake/LLVM 已安装；原生工作区构建、共享模块57项以及锁/存储/恢复三组7项均已通过。实际 Windows 浏览器/daemon 的文件工具、Job续跑、蓝图批准/拒绝与等待中刷新恢复也已实测。完整范围和失败修复记录见 `LOCAL_ACCEPTANCE_20261020.md`。

使用本轮已有前端产物，以下命令仅启动隔离演示和本地合成 provider，直接在当前本机 PowerShell 运行：

```powershell
$env:METTEUR_WEB_DIST = 'D:/BaiduNetdiskDownload/metteur-a7-work/build/local-acceptance-20261021-web-complete'
$env:METTEUR_VALIDATION_DIR = 'D:/BaiduNetdiskDownload/metteur-a7-work/tmp/manual-native-demonstration'
py -3.12 -X utf8 F:/space/06_Projects/Agent/workspace-review-2026-10-20/run_native.py node modules/webcore/scripts/run-nx-local.mjs --serve-only
```

按启动输出/ready.json中的实际 URL 和测试 workspace 打开界面；端口自动分配。在该终端按 Ctrl+C 停止演示。编译或重新链接 Windows 可执行文件前，先停止使用它的演示服务。本机验证启动器会激活已安装的工具，不要求修改全局 PATH。交接包 `local-tools/` 保存这些脚本；迁移机器时先调整脚本中的本机路径并安装相应工具。

这条演示入口不配置实际模型。真实双 DeepSeek 配对与完整链路已完成，总 US$5 合成数据授权未超额，逐次用量与估算见 EVALUATION_20261021.md。

## 收尾包直接启动与模型意见配置

包根 `Start-demo.ps1` 使用包内 runtime/windows 和 runtime/web；只需 Node 24，不需要 node_modules 或重新编译。此入口为免费合成演示。实际启动可显式运行 `metteurd --config <本机安全TOML> --data-dir <自己的数据目录> --listen-addr 127.0.0.1:<端口>`，再以 metteur-web 指向该 daemon 和生产 static-dir。不要从演示配置继承 synthetic provider，也不要把测试费用授权扩大为真实数据调用。

Addon 模型意见默认关闭。可信操作员需同时设置绝对路径 `METTEUR_PREVIEW_MODEL_PYTHON`、`METTEUR_PREVIEW_MODEL_REVIEWER`（source/scripts/nx_preview_model.py）、`METTEUR_PREVIEW_MODEL_SETTINGS`。settings 只引用 credential_config/credential_alias、允许模型 ID、同一 ledger_path、spending_limit_usd=5 和原 smoke imported_nano_usd=261900；不在 settings 放密钥。不要重建账本以绕过原 US$5 总额。renderer 仍需原有 Node/Chromium 绝对路径配置，保留沙箱。缺配置如实显示 not_configured；模型意见不是浏览器事实、人工验收或权限批准。

## 2026-10-21 便携启动更正

旧 2ada184 包启动器漏装 RPC 依赖，原仅 HTTP/ready smoke 不足，已撤回其无依赖启动结论。新包入口改用 runtime/fixture/scripts/run-nx-local.mjs（既有锁定 RPC/protobuf 依赖已打包，附许可证），仅需 Node 24。ready 只在依赖、工作区与蓝图初始化后写入。实际蓝图读取、ReadFile 两轮及历史恢复通过，无付费调用。不要继续使用旧包 Start-demo.ps1；原 zip 保留用于追溯。
