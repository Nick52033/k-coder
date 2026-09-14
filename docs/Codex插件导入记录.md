# 本机 Codex 插件导入记录

日期：2026-09-14。

用户要求直接将本机 Codex 插件导入 k-coder。本次向当前项目 `D:\code\k-coder\.k-coder\plugins` 导入 12 个插件，共 790 个文件。保留既有 `k-coder-test-plugin`，未写入客户端安装目录，未变更启用设置。首次发现默认禁用。

## 来源与导入结果

源缓存根：`C:\Users\mhke\.codex\plugins\cache`。

| 来源分类 | 插件 | 缓存版本目录 |
| --- | --- | --- |
| openai-api-curated | figma、superpowers | 1dc19589 |
| openai-bundled | browser、computer-use、unified-computer-use | 26.908.40834 |
| openai-bundled | codex-app-tools | 0.1.4 |
| openai-bundled | visualize | 1.0.37 |
| openai-primary-runtime | documents、pdf、presentations、spreadsheets、template-creator | 26.905.11957 |

每个插件根目录直接位于目标 plugins 目录下，并保留 `.codex-plugin/plugin.json`。全部来源与目标路径、文件清单、源文件 SHA-256 和适配后哈希记录在 `docs/Codex插件导入清单.json`。

## 最小元数据适配

复制时逐文件验证 SHA-256 一致，再对导入副本的 8 个 SKILL.md 作最小修改：

- presentations：Skill 名称从 `Presentations` 改为 `presentations`。
- spreadsheets：Skill 名称从 `Spreadsheets` 改为 `spreadsheets`。
- figma：缩短 figma-design-to-code、figma-generate-design、figma-generate-diagram、figma-generate-library、figma-swiftui、figma-use 的 description，以满足宿主 512 字节限制。正文保留，原始描述仍在 Codex 源缓存中。

最终再次核验 790 个源文件未变，目标文件除清单记录的 8 项元数据适配外与源文件一致。未复制链接或目录联接，未覆盖既有插件。

## 能力边界

这是文件导入和静态兼容检查，不等于所有插件已能执行：

- Skills 插件可以进入宿主发现流程；文档、表格和演示文稿等技能依赖的运行时、库、工具名称及远程连接仍须在 k-coder 中逐一验证。
- Figma 的 `.mcp.json` 声明 OAuth，当前宿主不支持；其 Apps 组件也不会执行。
- spreadsheets 的 Apps / Excel 实时连接不会随文件复制而迁移。
- browser 当前缓存没有可被 k-coder 发现的 `skills/*/SKILL.md`，且依赖 Codex 浏览器宿主。
- computer-use、visualize 依赖 Codex 专用宿主能力，复制技能说明不能提供这些能力。
- codex-app-tools、unified-computer-use 的 MCP 配置包含当前 k-coder 不支持的字段，如 `enabled`、`env`、工具配置；当前格式会被判为无效。保留原配置，不删去字段伪装兼容，不启动这些服务。

在 k-coder 打开本项目，进入“设置 → 插件管理”，刷新后按需启用并查看实际诊断。未尝试修改应用私有数据库或自动启用插件。

## 验证

- 路径与链接预检、790 个文件哈希复核通过。
- `pnpm build` 通过，存在既有大 chunk 提示。
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` 通过。
- `cargo check --manifest-path src-tauri/Cargo.toml` 通过。
- `cargo test --manifest-path src-tauri/Cargo.toml`：545 通过、3 失败。失败项为 `read_recovery_delivery_only_hard_stops_the_corrected_provider_batch`、`recovery_still_stops_varied_overlapping_reads_after_one_correction`、`semantic_read_tracker_recovers_once_before_stopping_overlap_loops`，与路线图 P10-175/P10-177 记录一致。
- 未启动 `pnpm tauri dev`，未实际验证桌面启用和执行路径，不声称桌面工作流完成。
- 未部署到 `D:\apps\k-coder`，未提交或推送代码。
