# ADR 0036：Windows 文件打开使用系统 Shell API

- 状态：已接受
- 日期：2026-08-15
- 关联：ADR 0014、P6-003、P10-082、P10-094、P10-104

## 背景

工作台会先把目标文件规范化为工作区内的绝对路径。Windows `canonicalize` 返回的路径通常带有 `\\?\` 或 `\\?\UNC\` verbatim 前缀；旧实现随后使用 `cmd /C start` 打开文件，并用 `explorer /select,...` 定位文件。

`cmd start` 会再次把目标当作命令行文本解析，而 Windows Shell 对 verbatim 路径的接受范围也不同于文件系统 API。有效目标因此可能被截断为 `\\`，并弹出“Windows 找不到 `\\` 文件”的原生错误框。该错误绕过应用错误状态，无法说明是哪个工作台操作失败。

## 决策

1. `open_workspace_file` 和 `reveal_workspace_file` 继续先通过工作区解析器规范化目标、解析符号链接或目录联接，并拒绝工作区外路径。
2. 授权检查完成后，Windows 边界可以为本次 Shell 调用临时把 `\\?\D:\...` 转为 `D:\...`，把 `\\?\UNC\server\share` 转为 `\\server\share`。该派生路径不得写回项目记录、会话绑定、工具参数或授权状态。
3. 文件打开和资源管理器定位统一使用已经注册的 `tauri-plugin-opener`。Windows 实现直接使用 Shell API，不再通过 `cmd /C start`、拼接命令文本或手工构造 Explorer 参数。
4. opener 错误沿类型化 Tauri 命令返回；文件预览在现有应用错误区域显示失败原因，不依赖原生“找不到文件”弹窗表达错误。
5. 其他平台继续由同一个 opener 契约选择系统实现，保持 `P6-003` 的跨平台功能不变。

## 影响

- Windows verbatim 路径不再进入 `cmd start` 的二次解析，文件打开失败不会产生目标为 `\\` 的系统错误框。
- 工作区规范路径仍是授权和持久化事实；Shell 兼容转换只发生在已经完成安全检查的最后边界。
- 系统打开由现有 Tauri 依赖负责平台差异，工作台不再维护一套独立的 Windows/macOS/Linux 启动命令。

## 验收

- 单元测试覆盖本地盘符、UNC 和已经是 Shell 兼容形式的 Windows 路径。
- 单元测试证明工作区逃逸在调用 opener 之前被拒绝。
- desktop/narrow E2E 覆盖 opener 失败的应用内反馈和资源管理器定位命令。
- 通过前端构建、Rust 格式/检查和全量测试，并启动原生 Tauri 窗口验证实际工作台路径。


