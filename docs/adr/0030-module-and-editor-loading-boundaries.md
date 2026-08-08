# ADR 0030：模块拆分与编辑器加载边界

## 状态

已接受，2026-08-08。

## 背景

Thread 协调、存储投影、Tauri 映射和前端事件 reducer 集中在少数超大文件中；Monaco 的整包 ESM 和所有 worker 进入按需编辑器 chunk，非 TypeScript 只读预览也具备加载 TypeScript worker 的能力。

## 决策

1. 模块拆分沿已经验证的职责进行，不改变公共协议或 JSONL 事实：thread operations、storage writer、history index、conversation event reducer 分别独立。
2. `commands/` 只保留请求校验和响应映射；`agent/` 负责 Turn 协调；`storage/` 负责 writer、重建和查询；前端 reducer 只投影事件。
3. Monaco 使用 ESM 最小入口并按语言动态注册 worker。非 TypeScript 预览不得构造 TypeScript worker；未打开编辑器时不得请求 Monaco 资源。
4. 拆分和包体优化必须保留编辑、只读、Diff、行号、高亮、保存和双视口交互。

## 后果

后续修改冲突和初次编辑器加载成本下降。代价是模块入口和 worker 注册更明确，需要覆盖动态加载失败及语言回退。

