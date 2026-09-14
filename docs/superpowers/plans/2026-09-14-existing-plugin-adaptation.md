# 现有七个插件适配与启用实施计划

> 执行：使用 superpowers:subagent-driven-development；用户已要求直接适配并启用，不提交代码。

**目标：** 逐个修复当前项目 browser、documents、presentations、record-replay、sites、spreadsheets、superpowers 的可执行本地流程，并经真实宿主启用。

**架构：** Skill 映射到 k-Coder 已有工具和本机已安装运行时；插件继续使用唯一 ExtensionService/PolicyEngine。interface 是展示元数据，不代表待执行组件；Apps 等真实未接通能力仍诊断，不伪造云服务可用。

**约束：** 源码仅修改当前仓库；保留已有工作区改动；不恢复用户移除的插件；不绕过路径、审批或凭据约束；不提交、推送或发布网站。

- [x] 宿主状态：先新增包含 interface 的 Skill 插件可加载、Apps 仍降级、路径逃逸仍拒绝的 Rust 回归，再修正计数。
- [x] 文档类：修复元数据，提供可运行的本地依赖定位与启动方式，实际生成并检查 DOCX/XLSX/PPTX，记录渲染/云导入限制。
- [x] browser/record-replay/sites：使用已有浏览器工具和受策略约束的 Shell，提供可执行录制/回放及本地网站流程；未提供的云连接器继续明确报缺失。
- [x] superpowers：映射 Skill 读取、计划与委派接口到实际工具，保留项目指令优先与默认不提交。
- [x] 验证：pnpm build、cargo fmt/check/test、适配脚本测试和插件专项；启动 pnpm tauri dev，实际读取 Skill、启用七个插件并刷新确认。全量 Rust 保留三个已记录的读取收敛失败，没有宣称全绿。
- [x] 更新路线图、ADR 0040、逐插件验收记录，注明安装部署与未解决边界。

## 执行记录

- 已检查任务的文件归属：宿主只改 Rust；文档类只改三个插件及独立 runtime 脚本；浏览器类只改三个插件及独立录制脚本；superpowers 由主任务处理，没有共享文件写入冲突。
- 决定在当前源码工作区执行：用户明确要求所有源码修改位于 D:/code/k-coder，且七个插件是未提交的当前内容。
- 原有安装进程来自 E:/Program Files/k-coder，与用户指定 D:/apps/k-coder 不一致；本轮先使用源码开发宿主验证和正常启用接口，不修改旧安装位置。
