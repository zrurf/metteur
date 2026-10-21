# 类型检查修复起点

> 2026-10-20：本机已复现并修复下列33条应用诊断，正常入口现检查app与所有
> 根配置；注入错误也会失败。详细证据见 `LOCAL_ACCEPTANCE_20261020.md`。
> 下文保留原始修复清单，供审计与回归参考。

## 已确认的问题
`modules/webcore/tsconfig.json` 的 files 为空，只有references。历史 `pnpm typecheck` 实际是 `vue-tsc --noEmit`，可能空跑。先修这一入口；根级命令成功不能继续当成app检查成功。

当前有效诊断命令（从modules/webcore执行）：

```sh
corepack pnpm exec vue-tsc --noEmit -p tsconfig.app.json --ignoreDeprecations 6.0
```

`--ignoreDeprecations 6.0`只允许暂时消除TS6对baseUrl的弃用阻断，不能消除实际类型错误。可优先正确迁移paths/baseUrl后取消该参数。Node配置应单独检查，或使用正确的build/references模式并核实覆盖了实际文件。不要简单改成另一个空跑命令。

## R1现有33条诊断分组
原始日志：包内 `evidence/ui-r1/integration-types-effective.log`；基线日志36条与比较结果同目录。

1. CodeEditor.vue：CodeMirror view 6.43.9与6.43.12混用，KeyBinding/EditorView私有字段身份冲突（3条）。先查frozen安装及依赖树，依据同一包版本统一，不用强转掩盖。
2. ExecTreeRow.vue：未使用Layers导入。
3. ChatComposer.vue、ContextMeter.vue、stores/chat.ts：core缺少ChatContextStats导出；ContextMeter region隐式any。核对实际定义与生成/导出边界。
4. ConciergePanel.vue：消息role被推为string，不满足ChatMessage联合类型；template中navigator不属于组件实例。正确类型化消息并将复制方法放script，处理剪贴板失败，保留原权限。
5. mock-gateway.ts：jobKillers成员缺失；ExecTreeData类型未导入。mock应匹配真实契约，不为通过测试伪造结果。
6. markdown.ts：MarkdownIt值/类型使用不一致（3条），按实际声明修复。
7. stores/config.ts：project_instruction_files和debounce_ms与真实配置类型不一致（8条）。先追踪Rust/proto/前端生成定义，不能删用户功能让错误消失。
8. ChatView.vue：未用workspace、todo/i/t隐式any、reasoning effort可能undefined。保留明确默认值与用户选择语义。
9. ExecutionPreview.vue：状态联合中不存在ready却进行比较；核对实际预览状态契约。
10. ExecutionView.vue：findLast需要正确ES lib目标，回调隐式any随之处理；Node/Chromium支持情况与编译配置对齐，不随意换target牺牲兼容。

以上清单是本地工作的入口，不能保证本地不同依赖环境恰好仍33条。重新执行记录实际结果；新增错误也需要修复。R1已经删去App中闲置chat变量、Memory中不可达状态比较，不要恢复它们。

## 完成标准
- 真正覆盖app以及必要Node配置的命令退出0；结果写入日志。
- package.json/verify.py/CI入口使用有效检查，不能保留假绿。
- 已有页面、接口与权限语义保持；修改生成文件需同步来源和生成步骤。
- 仅运行改动相关组件/接口的必要检查与生产构建。不因类型修复自动跑全量Rust、全E2E或平台矩阵。
