# 工作流预览确认记录

2026-10-22。**r2 / dw-ide-fixture-2** 三组均为 `APPROVED`，批准已呈现范围接入，保留现有 IDE 布局。最新密度修订 **r3 / dw-ide-fixture-r3** 已呈现，状态 `PREVIEW_READY / PRESENTED`，未代填用户具体版本签字。r1 未批准。正式入口行为及最终验收另行验证。

## r3：按用户反馈降低信息密度

用户最新原文：“我希望界面不要太多小字,信息量不要太大,要兼顾使用体验”。三组 r3 保留 r2 的 IDE 布局和已呈现操作，主文字/按钮 14px、状态 13px；来源、文件列表、预算/身份和次要草稿/运行操作默认折叠，Pause/Stop 仍直接可见。右下角 fixture 控件默认收起。恢复功能正式接入后，隔离构建隐藏重复真实恢复栏，避免同屏两个恢复入口。

入口：http://127.0.0.1:19085/（右下角切换 WF-PV-01/02/03）；r2 静态入口 19082 保留，不覆盖批准版本。r3 产物 `daily-workflow-evidence/20261022/preview-r3-final`，源码由本次完整密度修订提交绑定，逐文件摘要及源状态见 `preview-r3-clean-evidence/result.json`。

检查：最终隔离构建 exit 0；浏览器 IME/禁用发送/监督返回、再次冲突/create-only、结果加入原草稿/零模型、字体、默认折叠、明暗主题/1360/390px、原 IDE 视口几何、无外部请求/JS 错误均通过。命令 `node scripts/check-daily-ide-preview.mjs <证据根>/preview-r3-clean-evidence http://127.0.0.1:19085 r3` exit 0；定向 lint exit 0。截图已人工检查，未声称真实 Windows IME。

保留首轮失败：oxfmt 将旧 fixture 内联多语句格式化成不带分号的换行，Vue 编译失败；改为命名操作函数后构建通过。Run Options 折叠后测试旧定位器未先展开，修正测试操作后通过。对应 build/check 原日志和失败截图均保留。这些不影响 19082 冻结 r2 或真实生产入口。

用户对 r3 的具体版本确认：尚未取得；最新体验偏好已写入 DESIGN.md 并持续落实，r2 的已批准交互范围仍保留。最终真实入口验收未执行。

## 当前 r2：保留原 IDE 布局

用户对 r1 的原文反馈：“按照原设计,不要修改页面布局,原本就是模仿ide功能”。这不是对 r1 的批准。后续实施保留原活动栏、文件树、标签页、Monaco 编辑器、主聊天和 ExecutionLayout 的布局；不得按 r1 的独立页面壳接入。

三组 r2：
- WF-PV-01：http://127.0.0.1:19082/?preview=WF-PV-01
- WF-PV-02：http://127.0.0.1:19082/?preview=WF-PV-02
- WF-PV-03：http://127.0.0.1:19082/?preview=WF-PV-03

使用真正的 App、router、ChatView、EditorPane、ExecutionView/ExecutionLayout 和共享组件。只有隔离 Vite 配置 `vite.daily-ide-preview.config.ts` 在构建时做 fixture 适配；生产组件/导航/壳没有为预览修改。右下角折叠控件仅为夹具操作，不属于产品新增导航。构建强制 MockGateway，CSP 禁用连接，无 daemon/模型/真实文件能力。

范围：原聊天内关联运行卡与禁用发送、独立 Execution → Supervision、主草稿保持；原文本/画布编辑区的备份状态/恢复副本/三方冲突/引用/丢弃；原 Tasks 标题和页签中的来源/结果/分类异常；原有模型、权限和审批布局沿用，不宣称新模型调用或真实执行。

验证：app/Node typecheck、隔离构建、浏览器明暗主题/1360/390px/原壳视口几何/无外部请求/发送与 IME 合成序列/监督返回/再次冲突/create-only/结果入草稿均通过。证据在工作区 `daily-workflow-evidence/20261022/preview-r2-final-check`；result.json 包含候选、未提交源码状态和逐文件摘要。构建保存在 preview-r2，19082 监听此静态目录。源 commit 将由本次独立预览修订提交绑定，摘要为该记录权威。

首轮错误保留：原生 publicDir 未配置导致 favicon 构建失败；隔离 Tailwind 没有扫描原 App，程序交互检查未捕获视觉几何失败，截图复核发现后补 @source 和视口高度断言，复测通过。前两份截图/JSON 不作为可呈现 r2 的视觉通过证据。

用户 r2 确认原文：**“批准三组 r2，保留现有 IDE 布局”**。来源：本实施对话对三组 r2 确认问题的直接用户回复（question call_af586e2ad6794115bc0c8271f49a72ef）。确认主体：项目用户；记录时间 2026-10-22 12:48:10 +08:00（记录时间，不推定用户发送秒值）。源提交 `5e40cbf`，逐文件摘要见 preview-r2-final-check/result.json。允许接入仅覆盖上列实际呈现范围；不批准改变 IDE 总体布局，不替代正式行为、真实模型或最终验收。r1 静态产物仍留在19081，以下记录作为历史。

## r1 历史记录（未批准）

| ID / revision | 可操作入口 | 覆盖范围 |
|---|---|---|
| WF-PV-01 / r1 | http://127.0.0.1:19081/?preview=WF-PV-01 | 原会话 A/B 草稿、运行占用、禁用发送、独立监督、缺模型、断线未知、直接控制、完成不抢页面 |
| WF-PV-02 / r1 | http://127.0.0.1:19081/?preview=WF-PV-02 | 文本/无效画布/聊天/监督输入、精确 ACK、新编辑、恢复副本/双窗口/孤立稿、三方冲突、再次变化、引用选择、丢弃 |
| WF-PV-03 / r1 | http://127.0.0.1:19081/?preview=WF-PV-03 | 聊天/直接 Run 来源、精确交接、结束策略、四类结果、来源引用加入草稿、未知/预算/失效/撤权原因、独立审批 |

独立入口：`modules/webcore/src/previews/next/daily`，配置 `vite.daily-preview.config.ts`。fixture `dw-fixture-1`，无 daemon/模型/用户数据，CSP `connect-src 'none'`。生产 main/router 没有导入。静态包在工作区外证据根 `daily-workflow-evidence/20261022/preview-r1`，监听 19081；mock 基线实例 19080 与此不同。

源基线 commit `1b39ce8a8168a005d7f850dab19ccde2b1d49a09` 加预览文件；主组件 SHA256 `fd1ffcafc0bb10a75fb95e6dee8613d01b30540cc949f8c67e9d27e0f6d81bac`。构建脚本文件 `index-BKXsG8VC.js`。已通过 vue-tsc 与隔离构建。确定性浏览器检查：`node scripts/check-daily-preview.mjs F:/space/06_Projects/Agent/daily-workflow-evidence/20261022/preview-check-retest`，退出码 0；结果 JSON 含源码状态/摘要。验证明暗主题、1360/390px、reduced motion、输入/冲突/控制/结果操作、无外部请求或浏览器错误。未代替真实 Windows IME 或用户视觉确认。

首次类型检查发现三个无用图标导入，已移除；首次预览脚本 label 非 exact 同时匹配输入和按钮，已修正观察定位器。首轮 JSON 与工具输出保留。未修改产品通过预览测试。

请用户反馈：三组 r1 的信息组织、状态/操作和弹窗是否可接入；特别是原会话到监督的入口、备份/冲突选择、结果/恢复动作。可逐组批准；未获批组继续隔离。计划确认与具体界面确认不同，遵循原 SPEC C14。

确认主体：待用户。确认时间/反馈原文：未收到。最终真实入口验收：未执行。


## UI-CONSISTENCY r1 · 2026-10-23

用户要求统一主标题位置/字号及细节；具体冻结demo19101、源码65cc92f及测量/截图已呈现。用户明确回复“批准 UI-CONSISTENCY r1 接入（推荐）”。批准范围：保留IDE分区，统一PageHeader/18px主标题/间距，任务标题右侧操作，执行及设置共用控件和已展示的细节。没有新增权限或执行语义。

已接入生产19103并实际核验，原preview保留；见[UI_CONSISTENCY.md](UI_CONSISTENCY.md)。该确认不替代实际IME、OS睡眠/唤醒或8小时持续记录。
