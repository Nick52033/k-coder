# UI A 方向实施计划

目标：将主工作区整理为清晰、克制的专业工作台。用户已选择 A 方向并授权直接实施，按本计划在当前工作区执行，不提交代码。

设计依据：本会话确认的 A 方向；现有《UI-重设计方案.md》仅参考视觉方向，普通面板背景不要求两两 3:1。普通文字对比度检查按 4.5:1；控件必要标识与焦点按 3:1。

架构：只调整 React 展示样式与主题预览值。保持 1040px 消息最大宽度、现有面板拖动网格、工具折叠、审批与模型选择行为。共享工作区已有改动须保留，基线副本位于忽略目录 src-tauri/target/ui-a-baseline。

技术：现有 CSS 变量、React、Playwright；不增加依赖。

## 执行清单

- [x] 在 e2e/workbench.spec.ts 增加浅/深色界面可读性回归，覆盖导航文字、消息正文、输入焦点及缩放等效视口；先执行并确认失败原因。
- [x] src/App.css 更新原有字号和浅/深色语义变量；src/lib/theme.ts 同步预览色。新增 src/styles/workspace.css，通过 src/main.tsx 引入，集中调整侧栏、消息、输入区、状态和工作台，避免在大型样式文件末尾堆积覆盖。
- [x] 保持现有 DOM 和事件绑定，缩减装饰边框与聚焦抬升，保留失败、审批、运行中的状态提示。检查窄屏和面板拖动后的空间。
- [x] 更新原主题测试中的预期色值，运行专项及现有功能回归。执行 pnpm build、cargo fmt/check/test 和 git diff --check。全量遗留失败及基线复现见验证记录。
- [ ] 启动独立标识的 pnpm tauri dev，在真实 WebView 验证主题、输入、工作台、窗口最大化/还原和缩放等效视口；保留截图和验证记录。
- [x] 复查增量变更，更新路线图任务、当前位置、变更记录与验证文档；原生交互验收尚待执行，任务保留进行中。

## 验证入口

界面专项：`pnpm test:e2e --grep "professional workspace|appearance|themes|divider|workbench bounded|messages, composer|approval mode|unframed disclosure|soft turn continuation|subagent|robot progress"`，使用本机 Edge（PLAYWRIGHT_CHANNEL=msedge）。

原生启动配置存放 src-tauri/target/ui-a-validation.json，独立 identifier、开发端口和 WebView2 调试端口；测试不得读取正式用户配置、发送外部模型请求或部署正式安装目录。
