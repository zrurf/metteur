# DeepSeek 本地真实验收 · 2026-10-21

两款实际模型通过有界合成验收：`deepseek/deepseek-v4.1-flash`、`deepseek/deepseek-v4-pro`，均使用现有 CommandCode Chat Completions 连接。原密钥只在原配置中读取，未进入源码或交接包。

6 个 development × 2 模型 × 2 Prompt 的最新单元为 24/24；12 个冻结 held-out × 2 × 2 为 48/48（按实际回执更正断言）。development 来自 r2 的 20 单元和 dev-05 r3 的 4 单元。留出首轮原判 44/48，hel-07 的 4 单元实际均收到失败工具回执 `Launch is stale` 且执行数为 0；验收器只检查确认 RPC 抛错，误判了“接收选择”和“执行时复核”的分工。保留原判，离线重判，无新模型调用。没有根据留出集改 Prompt。

同一原生二进制、workspace 路径、文件初态、工具、权限、限额配对并交错先后顺序。只有旧 7 个 Prompt 和候选 runtime contract 的差别。候选 nx-m4-2 依据 development 的追加 echo 命令失败作适配；两组最终都通过，不宣称显著提升。

| 阶段 | 模型 | Prompt | 通过/单元 | 生成请求 | 输入/输出 token | 网关 TTFT 中位数(ms) | 峰值估算(US$) |
|---|---|---|---:|---:|---:|---:|---:|
| development | flash | baseline | 6/6 | 27 | 168456/3229 | 1407 | 0.054412 |
| development | flash | candidate | 6/6 | 26 | 169704/2868 | 1453.0 | 0.054353 |
| development | pro | baseline | 6/6 | 29 | 181362/3063 | 1804.5 | 0.251527 |
| development | pro | candidate | 6/6 | 26 | 166514/2917 | 1961.5 | 0.231350 |
| held_out | flash | baseline | 12/12 | 45 | 274419/6538 | 1436 | 0.090171 |
| held_out | flash | candidate | 12/12 | 47 | 301143/6736 | 1414.0 | 0.098426 |
| held_out | pro | baseline | 12/12 | 44 | 263371/4980 | 1797 | 0.367371 |
| held_out | pro | candidate | 12/12 | 43 | 274508/5598 | 1843.0 | 0.384519 |

TTFT 是受控 relay 接收到首个含内容/工具/推理字段的块的近似时间；relay 会缓冲响应，不是浏览器可见延迟，非流式保持 null。每个单元只有一次匹配执行，样本不支持显著性、普遍质量或速度结论。

还通过两模型的个人/项目 Prompt→记忆→确切蓝图批准→执行→原会话完整链；客服无 tools，监督 PauseRun 提案被单独拒绝后仍无动作。已安装签名 Addon 经离线 renderer 与真实模型意见完成绑定版本的独立验收流程，模型不能授权或替用户接受。

缓存专项的 cold 声明、完全相同 warm 请求、日期变化、资源修订变化共 8 单元在生产同等 1024 输出上限下通过；32/256 上限的首次探针出现未返回 ACK 的结果，均保留并计费。实际工具第二轮、宿主续跑和故意移除 cache/usage 字段也有记录。网关返回缓存字段不证明上游命中；缺字段的宿主视图保持 unknown，实际付费账本先记录供应商用量，未伪造为 0。

总计 **379 次生成请求**（含两次早期 smoke、所有首次失败和专项）；按公布峰值输入/输出单价、不假设缓存折扣，累计估算 **US$1.82139858**，低于 US$5 授权上限。供应商没有实际账单字段，不能把估算称为实际扣费。所有请求结果已结算，无未决 reservation/unknown；至此停止付费验收。

机器可读完整统计：`evidence/20261021-completion-model-report.json`；原留出判定和零调用重判同目录。全部逐次脱敏用量回执在交接包 `evidence/public-model-call-receipts-20261021.json`。浮动 alias 无不可变上游版本，这一边界继续保留。
