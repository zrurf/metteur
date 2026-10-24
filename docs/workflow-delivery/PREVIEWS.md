# 工作流交付隔离预览

2026-10-24，当前 revision r2。用户已明确回复“确认 r2，按这版接入”，覆盖 WD-PV-01/02 两组新增交互，进入真实服务接入阶段。隔离预览本身仍只用 fixture，不写真实文件、不运行或调用模型。

r1 用户反馈“还是最好满足我前面提出过的需求”，未计作批准。r2 将主要填写收敛为目标、范围、验收，模型/上限/证据/命令按需展开；复用 PageHeader，保持原 IDE。新增原聊天→模板→保存→确认→原聊天保留结果、文件编辑失败/未保存保护、文件修改使检查失效、任务/独立监督/原草稿往返。结果分别呈现执行状态、最终检查、人工接受；基线默认折叠。

r2 验证：preview-r2-types.log exit0；preview-r2-first 四组通过，连续流程组因 Stale 同时匹配测试场景下拉和状态标签而失败；限定结果区域后 preview-r2-continuity-retest 通过。所有运行 errors=[]、requests=[]。浅色结果/窄屏深色截图人工检查完成；不代签真实物理 IME 或模型运行。上述目录位于本页证据根。

- WD-PV-01：[模板、目标与检查、计划及精确启动确认](http://127.0.0.1:19110/src/previews/workflow/index.html?preview=WD-PV-01)。四类选择；保存失败保留输入；确认后资源变化阻断启动。复用原 App/导航/标签布局，组件位于 components/workflows。r2 获批后已接入正式入口，具体服务验收见 TASKS。
- WD-PV-02：[逐项结果、证据、人工审阅与后续草稿](http://127.0.0.1:19110/src/previews/workflow/index.html?preview=WD-PV-02)。选择 Failed/Missing/Skipped/Stale/Unknown/Conflict；前四类不得接受，未知结果先对账，冲突保留说明。接受不赋权、不执行；后续仅追加草稿。

确认范围是以上新增信息结构、操作顺序及窄屏/浅深色表现；未覆盖未展示的新增页面。实际宿主行为另做 L1/L3 证据，不以预览替代。

证据根 F:/space/06_Projects/Agent/workflow-delivery-evidence/20261024：preview-first 记录初次隔离路由误用 workspace guard 导致空白（无真实请求），修正仅限预览 route meta；preview-route-retest 四组通过，errors=[]、requests=[]；截图复核后修正文本框固定高度与按可用宽度分栏、暗色按钮，preview-layout-retest 定向通过。preview-types-first 曾有模板中 document 引用类型错误，改为组件方法后 preview-types-retest exit0。失败记录全部保留。

引用规则：SPEC §8.1.12 要求“用户查看指定 preview_id / revision → 有范围的明确确认 → 正式接入真实服务”。当前开工授权允许继续后端/独立测试/实验准备，不代填用户视觉确认。

用户确认记录：本对话对两组 r2 的明确回复“确认 r2，按这版接入”。不扩张为新模型预算授权或未展示页面的确认。
