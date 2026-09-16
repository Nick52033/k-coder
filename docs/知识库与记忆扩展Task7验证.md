# 知识库与记忆扩展 Task 7 验证记录：评测、迁移和桌面验收

- 完成日期：2026-09-16（**原生桌面验收一项未完成**，见 §3）
- 实施计划：`docs/superpowers/plans/2026-09-15-knowledge-memory-extension.md` 的 Task 7
- 权威设计：`docs/知识库与记忆扩展详细设计.md` §10.1、§10.2、§11 Phase E、§12
- 前置：Task 1–6 已完成

## 1. 固定评测集与回放

### 1.1 交付物

| 文件 | 角色 |
|---|---|
| `evals/knowledge-retrieval-baseline.json` | 固定语料（8 个文件）+ 固定查询集（8 条）+ 阈值，全部是数据 |
| `src-tauri/src/knowledge/evaluation.rs` | 回放执行器：建临时工作区 → 写语料 → 走真实 `KnowledgeService` 索引与检索 → 计算指标 → 比阈值 |
| `src-tauri/src/commands/mod.rs` + `lib.rs` | `run_knowledge_retrieval_evaluation`：同一个回放器，可从界面按需重跑 |
| 本文件 | 基线数字与复现命令 |

回放**不直接调用评分函数**，而是完整走一遍召回、融合、去重、邻接扩展与预算裁剪——踩能得到回归的是这些环节，不是几个纯函数。每条查询用**自己的 Turn**，所以 citation 解析与「跨 Turn 不可用」的规则是被执行的而不是被绕过的。

### 1.2 记录基线（2026-09-16，本机）

```text
recall@3=1.000  recall@5=1.000  mrr=1.000  citations=1.000
degraded=1.000  availability=1.000  p95=6ms  total=27ms
```

复现：

```powershell
cargo test --manifest-path src-tauri/Cargo.toml --lib retrieval_baseline -- --nocapture
```

### 1.3 每个指标到底在测什么（以及没测什么）

| 指标 | 定义 | 本次结果 | 诚实说明 |
|---|---|---|---|
| Recall@3 / Recall@5 | 相关来源出现在前 k 位（按首位命中算）的查询占比 | 1.000 / 1.000 | 语料刻意小；它的价值是**回归检测**：`filename-only` 一类的用例正是 Task 5 修掉的那个 `LIKE` 缺陷，谁把它改回去这里立刻变红 |
| MRR | 首位相关结果名次的倒数均值 | 1.000 | 语料里放了 `docs/retrieval-history.md` 作为共享词表的干扰项（`retrieval` + `scoring` 都会命中），所以 MRR 现在**是有区分力的**：标题/路径信号一旦退化，它会掉到 0.9375 以下并触发阈值 |
| 引用正确率 | 每个返回 citation 都能在本轮解析、且 path 与 revision 与结果一致 | 1.000 | 覆盖的是「citation 能不能落地」，不是「citation 内容是否真的支持结论」 |
| 降级率 | `retrievalMode != hybrid` 的查询占比 | 1.000 | **这是刻意的**：基线不带 embedding key 运行，钉住的是「没有凭据也必须可用」的词法底线，同时让数字离线可复现、不受第三方延迟与配额影响 |
| 可用性 | 降级后相关来源仍然返回的查询占比 | 1.000 | 这是降级路径真正要保证的东西 |
| 延迟 | 单次检索的 p95 与总耗时 | 6 ms / 27 ms | 只是本机冒烟上界（阈值 2000 ms），不是性能承诺 |

**混合检索（hybrid）的数字没有基线。** 它需要用户的 SiliconFlow 凭据与真实网络，属于原生验收范围，见 §3。

## 2. 全局检查与全量测试

| 命令 | 结果 |
|---|---|
| `npx tsc --noEmit` | 通过（零错误） |
| `npx vite build` | 通过（`✓ built in 15.47s`） |
| `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check` | 通过 |
| `cargo check --manifest-path src-tauri/Cargo.toml --all-targets` | 通过（零警告） |
| `cargo test --manifest-path src-tauri/Cargo.toml --no-fail-fast` | `--lib` **813 通过 / 3 失败**；集成目标 **31 项全绿**（mobile 6+19+6）；doc-test 0 |
| `pnpm exec playwright test e2e/knowledge-*.spec.ts e2e/memory-settings.spec.ts` | **8 通过**（4 个用例 × desktop/narrow） |

`--lib` 的 3 项失败与 Task 1–5 基线**逐项一致**（`read_recovery_delivery_only_hard_stops_the_corrected_provider_batch`、`recovery_still_stops_varied_overlapping_reads_after_one_correction`、`semantic_read_tracker_recovers_once_before_stopping_overlap_loops`），本轮没有新增失败。通过数变化：Task 4 = 744，Task 5 = 788，本轮 = **813**（+25：entities 21、端到端过期 1、评测基线 1、词表校验 2）。

## 3. 未完成：隔离 `pnpm tauri dev` 原生验收

Task 7 的第三项要求「启动隔离 `pnpm tauri dev`，验证记忆查看/审核/删除、检索引用、重启恢复」。**这一项没有做**，原因是具体且可复核的：

1. **仓库里没有可复用的隔离配置。** 既有的 `scripts/validate-*-native.cjs` 都假设一个已经起好的隔离宿主（Vite 1459 / WebView2 CDP 9399），而那个宿主是当时临时搭起来、验收后按惯例清理掉的（开发路线图相应条目写明了「隔离进程、临时脚本/配置和应用数据已清理」）。现在没有任何 `--config`、环境变量或脚本能把 `pnpm tauri dev` 起成一个隔离实例。
2. **不隔离就会写到真实应用数据里。** 按 `AGENTS.md`，客户端运行/安装目录固定为 `D:\apps\k-coder\`，直接 `pnpm tauri dev` 会对着用户正在使用的同一份应用数据跑迁移与写入。项目规则明确要求不要在安装目录制造改动，因此本轮没有这么做。
3. **要做成需要新建一套 CDP 驱动。** 即使补上隔离配置（独立 identifier + devUrl 端口 + `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9399`），§1–§2 之外的那几项（在桌面窗口里点关系审核、点记忆审核/删除、展开检索引用、杀进程重启后确认记忆与反馈仍在）都需要一个跑在真实 WebView2 上的驱动脚本；本轮没有为它腾出预算。

因此本轮**不得**声称桌面工作流已经验证。要补这一项，需要的步骤是：

1. 写一份临时隔离 Tauri 配置（独立 identifier ⇒ 独立应用数据根 + 独立 `devUrl` 端口）与一个 `.mjs/.cjs` 驱动脚本（Playwright `connectOverCDP`）。
2. `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>` 启动，先确认 `运行时就绪`。
3. 驱动验证：知识库页 → 检索控制台（真实索引 + 真实 citation 展开）→ 关系审核面板（真实 `propose`/`review` 往返）→ 记忆页（候选审核、删除、清空）→ 关闭进程 → 重启 → 确认记忆与反馈仍在、citation 已失效（进程内表）而反馈仍参与排序。
4. 记录 hybrid 模式的 Recall@k / MRR（带真实 key 跑一次评测命令），与 §1.2 的词法基线并列。
5. 清理：临时配置、临时脚本、隔离应用数据、进程。

## 4. 其它已知边界（Task 1–6 汇总）

- `knowledge.auto_search` 仍无消费者：检索只在模型工具与设置页显式触发，普通 Turn 不会自动检索。
- citation 是进程内表，满 `MAX_CITATIONS=500` 时整体清空而非按 Turn 回收。
- `project` / `workspace` scope 记忆可存储可管理，但不自动注入（缺宿主 project 身份）。
- `TaskSummary` 仍未接入维护提示词；`project`/`workspace` 维护不存在。
- 关系查询只有 subject 方向；实体类型只能改一次；非 ASCII 谓词被拒绝；`knowledge_facts` 缺 `source_revision_id` 索引。
- 维护调度是 5 秒轮询，应用未运行时不会补跑。
- 未提交、未部署。
