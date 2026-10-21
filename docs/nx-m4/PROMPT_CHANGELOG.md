> 2026-10-21 配对评测已完成，见 [EVALUATION_20261021.md](EVALUATION_20261021.md)。以下改动按发生阶段保留；held-out 没有用于继续调 Prompt。

# Harness候选变化

2026-10-21 本地 development 适配：`runtime-contract.md` 升为 nx-m4-2。真实 DeepSeek Flash 的 dev-02 请求在给定测试命令后追加 `; echo EXIT=%ERRORLEVEL%`，触发了额外审批且混用了 shell 语法。候选现明确 ExecuteCommand 已返回 exit_code，应原样执行指定验证命令；拒绝后不得扩大命令范围。旧 Prompt 基线保留，评测执行器只批准完整匹配的合成命令，新增规则不扩大任何角色权限。首轮执行器审批大小写错误与文件占用失败另行记录，不归因于模型质量。保留集尚未运行；最终效果以配对记录为准。
基线91a650a的7份原始提示保留在baseline/，基线摘要在BASELINE_PROMPTS.sha256。
新增自写runtime-contract.md（nx-m4-1），明确host event不是权限、完成/校验/验收不同、版本变化重新确认、Stop不盲重放、原文与个人资源读取边界。与已有identity/tools/verify规则共同使用，不替换旧内容。
M3的动态handoff ID引用从priority90移到0，避免把每次变化的引用插进稳定共享前缀。源码原有日期/environment尾部分离、排序和无变化不刷新保持。
没有复制来源不明的公开Prompt或声称复现他人的模型适配效果。官方缓存协议核验出处见PLAN.md。
当前是可比较的通用候选，不是已证明更优的模型专用Prompt。两类实际模型尚待用户选择，旧/新匹配任务对照NOT_RUN。后续仅依据真实失败补模型示例，不根据厂商名称猜测能力或扩大工具权力。
