# ADR 0056：操作系统级命令沙箱、平台差异与借鉴边界

- 状态：已接受
- 日期：2026-09-09

## 背景

k-Coder 已经具备完整的应用层安全边界：`tools/mod.rs`、`patch/mod.rs` 和 `workbench.rs` 三处独立实现路径规范化、`canonicalize` 与 `starts_with(root)` 越界拒绝；命令执行具备风险分级（`CommandRisk` 六档）、强制审批、超时（上限 1 小时）、取消令牌、进程树清理、输出脱敏与有界缓冲。ADR 0011 也明确 `full_access` 只替代逐次确认、不扩大 Capability。

但这些边界全部由宿主代码自身保证。命令进程一旦启动，就拥有完整的当前用户权限：

- `resolve_cwd()` 只约束工作目录，不约束命令内容，命令可以读写工作区之外的任意路径；
- 子进程继承宿主完整环境，只对请求体中的敏感键做过滤，没有 `env_clear()`；
- 没有 CPU、内存、进程数、磁盘写入和网络的任何上限；
- 提示注入、恶意依赖脚本和模型误判的破坏范围因此等于当前登录用户。

路线图 `P10-001`（威胁建模）与 `P10-002`（操作系统级命令沙箱）均未开始。本决策定义沙箱的边界、抽象、平台策略和明确不做的范围。

同时核查了两个参照项目：

- `cn-codex`：只实现了 Codex 沙箱协议的外壳。`sandbox_permissions` 和 `additional_permissions` 等字段仅用于生成审批弹窗与缓存授权档案，实际执行是裸 `Command::spawn()`，没有任何系统限制；`spawn_agent` 把 `--sandbox` 参数交给外部 Codex CLI，自身不实现。
- 官方 `codex`（本地源码）：抽象层很小（`SandboxManager` 四个方法、`SandboxType` 四个变体），平台实现约 4 万行，其中 Windows 侧约 2.1 万行。Linux 的 landlock 只用于网络限制，文件隔离实际依赖 bubblewrap（约 2,700 行 argv 构造）。Windows 侧大量代码用于 deny-read、沙箱专用账号、提权后端与私有 desktop 等平台坑。

结论是：抽象值得借鉴，平台实现必须做子集裁剪。

## 决策

### 1. 沙箱只保护命令执行，不改变文件工具边界

沙箱只作用于会创建进程的能力：`run_command`、MCP、Hook 和子智能体派生的命令。`read_file`、`list_directory`、`search_repository` 和 `apply_patch` 继续由既有 `Workspace` 边界保护，不进入操作系统沙箱，也不因为引入沙箱而放宽任何现有校验。

工作台终端（PTY）是用户自己的交互操作，架构文档已明确它不经过 `PolicyEngine` 审批。沙箱同样不作用于 PTY，或只使用显式标注的宽松 profile；用户手工执行的命令不得被静默拦截。

### 2. 统一抽象：SandboxProfile 与后端能力协商

新增 `execution/sandbox.rs`：

```rust
pub struct SandboxProfile {
    pub filesystem: FsPolicy,       // WorkspaceOnly / WorkspacePlusTemp / Full
    pub network:    NetworkPolicy,  // Deny / AllowHosts(Vec<String>) / Allow
    pub resources:  ResourceLimits, // 墙钟超时、CPU 时间、内存、最大进程数
    pub ui:         UiPolicy,       // 是否允许弹窗、剪贴板、桌面切换
}

pub enum SandboxCapability {
    Full,
    Partial { filesystem: bool, network: bool, resources: bool },
    Unsupported,
}

pub trait SandboxBackend {
    fn capability(&self) -> SandboxCapability;
    fn apply(&self, cmd: &mut Command, profile: &SandboxProfile) -> Result<SandboxHandle>;
}
```

唯一的进程侧挂点位于 `configure_process_group()` 之后、`spawn()` 之前，其余模块不感知沙箱细节。

`ExecutionWorkspacePolicy::authorize` 在返回 `requires_approval` 的同时产出 `SandboxProfile`；MCP、Hook 和子智能体按 ADR 0003 复用同一条链条，不新增第二个决策源。

### 3. 能力不足时 fail-closed

对齐 ADR 0003「配置错误一律关闭失败而非降级放行」：

- 后端能力无法满足 profile 要求（例如要求 `network: Deny` 而平台为 `Unsupported`）时，拒绝执行并返回明确中文诊断，不静默降级为无沙箱运行；
- 用户可在设置中显式开启「隔离不可用时仍继续」，该选择必须写入审计事件；
- 沙箱不随 `full_access` 解除。延续 ADR 0011 的口径：完整访问只替代逐次确认，不扩大能力，也不关闭隔离。

沙箱与审批是正交的两个维度。沙箱级别越高，可以免确认的命令类别越多；第一版保持现有审批规则不变，后续按测量数据收敛。

### 4. 沙箱拒绝是独立结果类型，禁止暴露原始系统错误

沙箱拒绝必须区别于「命令执行失败」和「用户拒绝」。否则模型会误判为命令不存在或环境问题，进而反复尝试绕过（例如 `curl` 被拦后改用 `Invoke-WebRequest`，再改用 Python）。

- 拒绝返回独立的类型化结果，附带固定中文文案与明确补救指引；
- 禁止把 `ex.Message`、Windows 错误码或原始 stderr 回传给模型，沿用既有对外文案约定；
- 工具描述和运行时 system prompt 必须说明当前生效的沙箱边界，让模型第一次就选择正确的执行路径；
- 同一策略在同一 Turn 内的重复拒绝只保留一次提示，拒绝输出受现有截断与折叠机制约束，避免污染上下文并提前触发 Compaction。

### 5. 平台策略与差异

| 平台 | 后端 | 文件 | 网络 | 资源与进程树 | 需要提权 |
| --- | --- | --- | --- | --- | --- |
| Windows | Job Object + 受限令牌 + deny-write ACL | 部分（工作区外写保护） | 可选（防火墙 COM 或 WFP） | 是 | 否，网络项需要一次提权 |
| macOS | `sandbox-exec` 与 SBPL profile | 是 | 是 | 部分 | 否 |
| Linux | landlock（文件）与 seccomp（网络） | 是 | 是 | 是 | 否 |
| 其他 | 无 | — | — | — | — |

Windows 优先，因为当前主目标是 Windows。第一版只实现三件事：

1. **Job Object**：`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 保证宿主异常退出时进程树必死（比现有 `taskkill /T /F` 可靠）；`JOB_OBJECT_UILIMIT_*` 限制剪贴板、桌面切换、退出 Windows、全局钩子和系统参数；CPU 与内存上限、活动进程数上限。不需要提权，不影响 PATH 和临时目录。
2. **受限令牌与 capability SID**：`CreateRestrictedToken` 去掉管理员 SID 与最大权限，按工作区生成随机 capability SID 做跨工作区隔离。
3. **deny-write ACL**：对工作区之外下拒绝写 ACE，对 `.k-coder` 等宿主目录额外下拒绝写 ACE，配合 allow ACE 保证幂等。

Unix 后端在 Windows 版本验收后实现：macOS 用 `include_str!` 内嵌 SBPL 模板并配合 `sandbox-exec -DKEY=value` 参数注入；Linux 用 landlock 限制文件系统、seccomp 限制网络 syscall，并显式记录 landlock 只解决文件与网络，不做 bubblewrap 级别的挂载隔离。

### 6. 分阶段落地

| 阶段 | 内容 |
| --- | --- |
| a | 威胁建模（对应 `P10-001`）与 `SandboxProfile`、`SandboxBackend`、能力协商、审计事件、默认 fail-closed 及单测 |
| b | Windows Job Object 后端：进程树生命周期、UI 限制、资源上限 |
| c | Windows 受限令牌、capability SID 与 deny-write ACL |
| d | 网络策略：Windows 防火墙 COM 规则或 WFP 二选一，显式放行链路与审计 |
| e | macOS 与 Linux 后端、平台差异矩阵文档、三平台 CI 验证 |

阶段 a 到 c 完全不需要提权，也不创建任何 Windows 账号。

### 7. 实现位置与当前状态（`P10-002a`）

- 抽象位于 `src-tauri/src/execution/sandbox.rs`：`SandboxProfile`、`SandboxCapability`、`SandboxBackend`、`SandboxGate`、`SandboxAudit`。
- **profile 由执行侧派生，不接受模型参数**：`CommandRuntime::start` 用 `assess_command(program, args)` 得到 `CommandRisk`，再由 `SandboxProfile::for_risk` 派生期望隔离强度。策略引擎另提供 `ExecutionWorkspacePolicy::sandbox_profile` 供审计和界面复用同一结论，模型既不能伪造也不能绕过。
- `Network` 风险的命令派生 `network: Allow` 而不是 `Deny`：当前平台后端还无法真正阻断网络，声明一个兑现不了的约束只会让所有命令被 fail-closed 挡住。
- 唯一内置后端 `NoSandboxBackend` 明确报告 `Unsupported`。因此**本阶段降级默认开启**：需要隔离但无法兑现时继续执行命令，并写入 `SandboxAudit { outcome: Degraded, reason }`。关闭降级（`with_degraded_execution(false)`）时按 ADR 的要求真正拒绝执行。
- `P10-002b` 引入真实 Windows 后端之后，必须把 `SandboxGate::allow_degraded` 的默认值改为 `false`，让能力不足真正关闭失败。
- 审计事实通过 `CommandSessionView.sandbox` 暴露给界面与日志；后端内部错误只进入日志，回传给模型的是固定中文文案。

## 兼容与非目标

- 明确**不做** codex 的 deny-read 子系统：它依赖提权后端，约 1,450 行，收益与成本不成比例。
- 明确**不做**沙箱专用 Windows 账号、DPAPI 凭据、隐藏用户和提权后端 IPC：这些只在需要完全独立身份或托管网络时才必要。
- 明确**不做** ConPTY 桥接、私有 desktop 和命令行包装器二进制，除非实测证明受限令牌下 PowerShell 确实无法启动。
- 明确**不做** bubblewrap、容器和 WSL2 后端。
- 沙箱不保护工作区内的自毁行为（`rm -rf` 工作区本身是合法写入），该风险继续由 Git 快照与变更回滚承担。
- 沙箱不防御通过工具输出把秘密回传给模型，该风险继续由输出脱敏与审批承担。
- 沙箱不影响 `read_file`、`list_directory`、`search_repository`、`apply_patch` 的既有行为。

## 对协议与持久化的影响

- 命令执行事实新增沙箱维度（生效后端、profile 摘要、拦截原因），旧会话缺少该字段时取默认值，历史恢复不得失败；
- 前端命令卡片显示隔离状态（例如「在隔离环境中运行、网络已关闭」），拦截事件进入时间线但受折叠与去重约束；
- 协议新增字段必须可缺省，旧客户端与新后端双向兼容。

## 验收

- Rust 覆盖 profile 生成、能力协商矩阵、fail-closed 分支、Job Object 句柄生命周期、ACL 幂等、拒绝文案不含原始系统错误、旧事件缺少沙箱字段时的恢复。
- 前端覆盖隔离状态展示、拦截事件折叠与去重、窄屏布局。
- Windows 真机验证：工作区外写入被拒、工作区内写入正常、宿主异常退出后进程树被清理、网络关闭时外部访问失败且拦截被记录。
- `pnpm build`、`cargo fmt --check`、`cargo check`、`cargo test` 全部通过。
