# ADR 0008: 屏幕区域截图功能

## 状态

已采纳

## 背景

用户希望在 k-Coder 中实现微信/QQ 那样的屏幕区域截图：框选屏幕任意区域后，截图可作为图片附件粘贴给 AI，
让 AI 能看见报错信息、UI 界面等内容。

项目此前的能力：
- 已支持图片附件（`ImageAttachment`），前端通过 `attachments`（`kind === "image"`，`content` 为 dataUrl）
  把图片传给后端 `runTurn`。
- 已有 `browser_screenshot` 工具，但只能截浏览器打开的页面，无法捕获任意屏幕区域。

## 决策

### 1. 后端：`capture_screen` 命令

新增 Tauri 命令 `capture_screen`，用 **`xcap`** crate 捕获主显示器画面，编码为 PNG base64 dataUrl 返回。

- `xcap` 跨平台（Windows / macOS / Linux），API 返回 `image::RgbaImage`。
- 截图是阻塞操作，用 `tauri::async_runtime::spawn_blocking` 包裹避免阻塞 UI。
- 返回结构 `CaptureScreenResult { dataUrl }`，格式为 `data:image/png;base64,...`。
- PNG 编码使用 `xcap::image::ImageFormat::Png`（与 xcap 内部 image 版本一致，避免版本冲突）。

### 2. 前端：`ScreenshotOverlay` 组件

新增全屏截图遮罩组件，实现"框选 + 裁剪"：

- 截图按钮触发 `captureScreen()`，成功后显示遮罩。
- 遮罩用 `<img>` 展示整屏截图，用户拖拽框选矩形区域。
- 通过 `canvas` 按显示缩放比例裁剪选中区域，`toDataURL("image/png")` 生成裁剪结果。
- 确认后把裁剪结果作为 `AttachmentContent`（`kind === "image"`）加入附件列表，走已有的图片附件链路。

### 3. 附件类型补齐

`AttachmentContent` 需要 `size`、`truncated` 字段，截图裁剪后按 base64 长度估算 `size`，`truncated: false`。

## 后果

### 正面

- 用户可截取任意屏幕区域发给 AI，扩展现有图片附件能力。
- 复用已有的附件/`runTurn` 链路，无需改动消息发送逻辑。
- 跨平台（xcap 支持三大桌面系统）。

### 负面

- 目前只捕获主显示器；多显示器下其他屏幕需后续扩展。
- 截图不经过系统级"截图工具"交互（如放大镜、精确取色），仅提供框选裁剪。
- Windows 上截图时需要应用窗口保持前台，否则可能捕获到其他窗口。

## 参考

- k-Coder ADR 0007（plan-collaboration-mode）
- k-Coder 现有 `browser_screenshot` 工具（`src-tauri/src/advanced/browser.rs`）
