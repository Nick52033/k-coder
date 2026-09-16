# 知识库与记忆扩展 Task 3 验证记录：上下文编排与工作记忆

- 完成日期：2026-09-15
- 实施计划：`docs/superpowers/plans/2026-09-15-knowledge-memory-extension.md` 的 Task 3
- 权威设计：`docs/知识库与记忆扩展详细设计.md` §4.3、§5.1、§5.2、§10.1
- 前置：Task 1（数据库迁移与事件投影）、Task 2（基础记忆领域服务）已完成

## 1. 交付内容

### 1.1 新增 `src-tauri/src/context/assembler.rs`（上下文编排）

设计 §5.1 的优先级被写成枚举的声明顺序，而不是散落在调用点的 `if`：

```text
安全策略 > 用户明确规则 > 项目 AGENTS.md > 用户确认的约束记忆
        > 项目事实记忆 > 当前工作记忆 > 知识库引用 > 模型推断
```

- `ContextTier`：8 级，判别值即排序键；`CONTEXT_TIERS` 常量供穷尽遍历与测试断言。
- `ContextFragment` / `FragmentSource`：片段携带来源种类、宿主 ID、`revision` 与规范 scope 串。
  构造器覆盖每个层级（`safety_policy` / `user_rule` / `project_rule` / `from_memory` /
  `task_summary` / `knowledge_citation` / `model_inference`）。
- `ContextAssembler::assemble`：稳定排序 → 敏感过滤 → 单片段截断 → 预算裁剪。
- `AssembledContext`：`sections`（按层合并的正文）、`injections`（每个候选的审计记录）、
  `over_budget`、`injected_chars`、`estimated_tokens`，以及 `render()` / `trimmed()` /
  `audit_summary()`。
- `memory_fragments(records, now_ms)`：从持久化行构造可注入片段，并做组装期 TTL 过滤。

### 1.2 新增 `src-tauri/src/context/task_summary.rs`（任务结束压缩）

设计 §4.3 要求工作记忆与经验只保存结构化摘要。`TaskSummary` 是唯一入口：

- `new` / `push_step` / `push_tool_step` / `push_file` / `push_follow_up` / `compress`。
- `sanitize()` 依次执行：折叠 `data:image/...;base64,...` 为 `[image]` → 归一空白 →
  过运行时脱敏 → 按字段截断。
- 上限：单字段 240 字符、单列表 6 项、渲染结果 900 字符；`push_tool_step` 只引用工具输出的
  前 2 行。

### 1.3 Compaction 边界加固（`src-tauri/src/context.rs`）

设计 §10.1 要求「验证 Compaction 不复制图片、密钥或完整工具输出」。验证过程发现压缩摘要此前
只做长度收敛、不做脱敏，因此补了三处脱敏（不是只补测试）：

1. `new_summary_text` 的 `Text` / `UserContent` / `ToolResult` 分支逐条 `redact`。
2. `important_tool_observations` 的高信号行在拼接后 `redact`。
3. `summarize_large_tool_result` 在长度判断**之前**脱敏，因此短工具结果同样被清理；同时
   `render_summary` 在渲染边界再做一次 `redact`，覆盖从事实日志恢复的旧摘要。

图片语义不变：摘要只记录 `[N image attachment(s)]`，像素仍以「保留一个有界上传批次」的既有
方式留在历史里，不进入摘要文本。

### 1.4 记忆注入接线（`src-tauri/src/commands/mod.rs`）

`live_runtime_instruction_provider` 现在把 Task 2 记忆经组装器注入 `<memory>`：

- 新增 `assemble_memory_context(memory, logger, thread_id)`，读取 `user` 与 `thread:<threadId>`
  两个 scope 的 active 记忆，交给 `ContextAssembler::default()` 组装，返回渲染后的文本。
- 门控：`MemorySettings::enabled` 为假时完全不注入（设计 §12.4；用户介导的查看/编辑/审核/删除
  不受影响）。
- 审计：每次注入写一条 `memory_context_injected` 日志，字段是 `threadId` 与有界的
  `audit_summary()`（含 `included`、被裁剪的 ID 列表、被过滤的 ID 列表、`over_budget`、
  估算 token）。scope 读取失败或设置读取失败写 `error` 日志并且不中断 Turn。
- 旧 `advanced/memory` 存储保持原样：它有自己的 16 KiB 预算与自己的 `enabled` 门，作为独立
  文本块保留在 `<memory>` 前部，避免被 12,000 字符的组装预算挤掉（回归风险）。

## 2. 关键设计决策

### 2.1 预算裁剪的顺序：先保安全，再保规则，最后才动记忆

`SafetyPolicy` / `UserRules` / `ProjectRules` 标记为 `is_required()`，永不被裁剪、永不被截断；
它们超预算时置 `over_budget = true` 但仍然注入。理由是设计把「`context` 不覆盖安全策略」列为
硬边界：一个能靠耗尽预算来削弱边界的组装器本身就是缺陷。其余层级按优先级从低到高被裁剪，且
每个被裁剪的候选都留下 `ContextInjection { state: Trimmed }`。

### 2.2 层内顺序由调用方决定，跨层顺序由枚举决定

`assemble` 只做「按 tier 的稳定排序」，层内保持调用方传入顺序；`memory_fragments` 负责层内
的确定性排序：`(tier, updated_at_ms DESC, id ASC)`。这样既保证「同样输入 → 逐字节相同的
请求」，又不牺牲新近记忆优先的语义。

### 2.3 经验记忆映射到项目事实层

设计 §5.1 的层级列表没有单独的「经验」层，而 §4.3 又要求经验使用 `memory_type=experience`。
本 Task 把 `experience` 与 `fact` 一起放进 `ProjectFactMemory`，层内按新近度排序，并在代码注释
里写明这是对 §5.1 的解释而非新增层级。`preference` / `instruction` / `constraint` 进
`ConstraintMemory`，`work_state` 进 `WorkStateMemory`。

### 2.4 TTL 在组装期重算一遍，而不是只信 `expires_at_ms`

`ContextFragment::from_memory` 同时检查两件事：`expires_at_ms <= now` 直接排除；
`work_state` / `experience` 再按 `created_at_ms + 设计 TTL` 判活（14 天 / 180 天，恰好到期即算
过期）。理由：组装期规则不能因为某行写在规则之前就失效，`expires_at_ms` 为空的行也必须能被
14 天 TTL 兜住。

### 2.5 敏感过滤是「双层防御」，不是重复实现

写入侧（Task 2）已直接拒绝凭据形态内容（`MEM_SECRET_REJECTED`，不落库），组装侧再做一次
`detect_sensitivity == SecretCandidate → 整片丢弃`。第二层不是冗余：它拦住的是规则生效之前写入
的旧行、外部导入的投影，以及未来放宽写入侧策略的情形。测试用 `MemoryRepository::append` 直接
写一行 `sensitivity = "normal"` 的凭据内容来证明第二层真的生效。

### 2.6 注入范围只覆盖宿主能推导的 scope

只自动注入 `user` 与 `thread:<threadId>`。`project` / `workspace` scope 需要宿主生成的
project 身份，而当前运行时只有 thread id（设计 §7.1 明确要求 scope 由宿主生成，不允许模型提交）。
本 Task 不发明 project 身份，因此这两类记忆**可存储、可管理，但暂不自动注入**——这是有意的范围
边界，不是遗漏。

### 2.7 旧记忆存储不并入组装预算

旧 `advanced/memory` 的 `context()` 上限是 16,000 字符，比整个组装预算（12,000）还大。若把它
作为普通片段塞进组装器，它会吃掉全部预算并把新记忆全部裁掉。因此保留为独立块，仅在
`<memory>` 内前置拼接，行为与改动前逐字节一致。

## 3. 测试

新增 26 项回归：

| 文件 | 数量 | 覆盖 |
|---|---|---|
| `context/assembler.rs`（新增） | 13 | 层级顺序与覆盖、确定性、预算裁剪与审计、必保层溢出、UTF-8 边界截断、敏感过滤、TTL 与状态过滤、层内新近度、未知枚举防御、规范 scope |
| `context/task_summary.rs`（新增） | 8 | 确定性渲染、空摘要、图片折叠、凭据脱敏、完整工具输出不复制、字段与列表上限、空白归一、进工作记忆层 |
| `context.rs`（新增） | 3 | 摘要不含图片载荷、压缩全链路脱敏、不复制完整工具输出 |
| `commands/mod.rs`（新增） | 2 | 注入门控/顺序/跨线程隔离、凭据行双层防御 |

专项命令（本轮测得时刻 2026-09-15 23:5x）：

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --lib context::
cargo test --manifest-path src-tauri/Cargo.toml --lib memory_context
```

`context::` 40 项全绿；`memory_context` 2 项全绿。

## 4. 质量门槛

| 命令 | 结果 |
|---|---|
| `tsc && vite build`（本机 `pnpm` 因 corepack 链接不可用，改用等价命令） | 通过 |
| `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` | 通过（先 `cargo fmt` 修掉 4 处） |
| `cargo check --manifest-path src-tauri/Cargo.toml --all-targets` | 通过 |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib` | **712 通过 / 16 失败** |

16 项失败与 Task 1/Task 2 基线**逐项一致**（3 项既有读取收敛 + 13 项本机符号链接创建异常），
没有新增失败。净增 26 项通过（Task 2 为 686，本次 712）。

## 5. 未完成与已知问题

- **未做原生 `pnpm tauri dev` 验收**。本轮改动里记忆注入会在每个 Provider 请求上触发一次
  `memory_context_injected` 日志，但桌面路径（设置页开关 → 注入 → 观察日志）没有实际跑过，
  不得声称桌面工作流已完成。
- **`project` / `workspace` scope 记忆不自动注入**（见 §2.6）。需要先定义宿主 project 身份。
- **安全策略与规则层尚未由组装器统一编排**。`<identity>`、`<workspace>`、扩展规则仍由
  `build_system_prompt` 与 `extensions` 各自拼接，组装器的这两个层目前只有单元测试覆盖。
  统一到同一个组装器属于提示词架构改动，本 Task 不做。
- **`TaskSummary` 尚无生产调用方**。按计划它是 Task 4「维护 Turn 与 Dream」的输入
  （`bounded task summaries`），本 Task 只交付类型、压缩规则与测试，不实现自动捕获。
- **组装器输出尚未进入 Provider 请求的持久化审计**。审计目前只写运行日志，没有落盘到 JSONL；
  设计 §5.1 只要求「记录」，日志满足字面要求，若要可回放需在后续 Task 决定事件形状。
- 未提交、未部署。

## 6. 工作区状态

- 新增：`src-tauri/src/context/assembler.rs`、`src-tauri/src/context/task_summary.rs`。
- 修改：`src-tauri/src/context.rs`（模块声明 + 脱敏加固 + 3 项测试）、
  `src-tauri/src/commands/mod.rs`（注入接线 + 2 项测试）。
- 工作区是移动靶：验证期间观察到并行会话正在修改 `src/App.tsx`、`src/App.css`、
  `e2e/workbench.spec.ts`，与本次改动无关；上述质量门槛结论只对应本记录标注的时刻。
- 未执行 `git commit` / `git push`。
