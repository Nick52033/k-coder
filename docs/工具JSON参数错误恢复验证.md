# 工具 JSON 参数错误恢复验证

## 问题与范围

2026-09-14 的“新会话”（953835b9-8318-4f1f-ba14-4d8954251324）在 TokenHub/deepseek-flash 的 request_user_input 调用中收到提前闭合 questions 数组的 JSON，严格解析报 trailing characters。解析拒绝是正确行为；原有字符串分类把 Provider 错误标成 invalid_input / retryable=false。

初次排查对手动重试门禁的判断有误：当前后端实际根据 TurnFailed / TurnCancelled 终态允许重试，不按 retryable 字段拒绝。此次通过真实重试链路验证恢复，不修改历史事件。

## 实现

- 三个接收工具参数字符串的适配器使用 InvalidToolArguments 独立错误类型，保留严格 JSON 解析。
- InvalidToolArguments / InvalidResponse 按类型映射 provider_invalid_response / protocol / retryable=true；真正的无效用户输入保持原分类。
- 流前与流内统一协议重试匹配，最多 5 次；已有响应输出时不重放，不切换备用供应商。保留取消、用量累计和工具执行边界。
- 失败 details 记录 protocolRetries 与 outputAlreadyStarted，不包含完整参数。Chat Completions 删除 Raw arguments 错误正文。
- 补充错误分类、参数拒绝、脱敏、备用供应商边界、流前重试、重试上限与手动恢复、取消回归；既有用量及已有正文不重放回归改用类型化错误。

## 验证

`pnpm build`、`cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`、`cargo check --manifest-path src-tauri/Cargo.toml` 通过。Rust 全量为 542 通过、4 个既有失败（3 个读取收敛旧断言、1 个本地 ripgrep 环境超时），新增相关测试全部通过。

原生复现脚本 `scripts/validate-tool-json-native.cjs` 通过：独立应用标识 com.kcoder.validation.tooljson、Vite 1477、WebView2 CDP 9417 和本地 HTTP/SSE 模拟 Provider，验证了格式错误自动恢复、三题提交、已有输出不重放、手动重试、6 次上限、取消和无原始参数泄漏；未调用真实模型。

## 交付状态

仅修改 D:\code\Nick\k-coder 源码仓库；保留既有日志、读取收敛等未提交改动。不提交、不推送、不部署到 D:\apps\k-coder。正式客户端需要后续构建部署才会使用本次修复。
