# k-Coder

k-Coder 是一个使用 Tauri、Rust、React 和 TypeScript 构建的本地优先编程智能体桌面应用。

项目会先实现一个小型、可测试的智能体运行时，再按受控阶段逐步发展成成熟的编程智能体工作台。完整开发顺序和验收门槛见 [开发路线图](docs/开发路线图.md)。

## 当前状态

- 阶段：`Phase 1 - 流式对话运行时`
- 运行时：Tauri 2 + Rust
- 界面：React + TypeScript + Vite + Zustand
- 下一项任务：确定性的 `FakeProvider` 和 Provider 测试夹具

## 本地开发

环境要求：

- Node.js 22+
- pnpm 10+
- Rust 1.85+
- 当前操作系统所需的 Tauri 2 构建环境

安装依赖并验证：

```powershell
pnpm install
pnpm build
cargo check --manifest-path src-tauri/Cargo.toml
```

启动桌面应用：

```powershell
pnpm tauri dev
```

## 项目文档

- [开发路线图](docs/开发路线图.md)
- [运行时架构](docs/架构.md)
- [技术选型决策](docs/adr/0001-technology-stack.md)
- [智能体协作规则](AGENTS.md)

## 项目原则

- Rust 负责智能体运行时、工具、权限、存储和模型接入。
- React 负责展示和用户交互，不负责智能体决策。
- 工具默认禁止访问已选工作区之外的资源。
- 每个路线图阶段通过验收门槛后，才能开始下一阶段。
- 密钥、本地对话、构建产物和运行时数据库不得提交到仓库。
