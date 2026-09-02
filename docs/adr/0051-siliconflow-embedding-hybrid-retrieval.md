# ADR 0051：硅基流动 BGE-M3 与本地 SQLite 混合检索

- 状态：已接受（设计阶段，尚未实现）
- 日期：2026-08-31

## 背景

k-Coder 需要把用户明确选择的工作区文档切片后用于知识检索。项目没有本地 embedding 模型，也不希望第一期引入 PostgreSQL/pgvector、常驻向量服务或额外运维组件。纯 FTS5 能覆盖精确关键词，但对同义表达和自然语言问题召回不足；同时，知识内容和 API Key 必须保持清晰、可审计的外发边界。

用户指定使用硅基流动的 BGE-M3。官方 Embeddings 接口支持字符串或字符串数组输入，数组最多 32 项；BGE-M3 单项输入上限为 8192 tokens，`encoding_format` 支持 `float`/`base64`，`dimensions` 仅适用于 Qwen/Qwen3 系列。

## 决策

1. 第一期固定 SiliconFlow 能力 profile：
   - endpoint：`https://api.siliconflow.cn/v1/embeddings`
   - model：`BAAI/bge-m3`
   - `encoding_format`：`float`
   - 鉴权：只使用官方文档列出的 `Authorization: Bearer <账户 API Key>`；不在第一期开放同页列出的 `x-api-key` Header 切换
   - API Key：操作系统凭据槽 `embedding-api-key:siliconflow`
2. endpoint、model、编码格式和请求体中的能力字段由后端固定；设置页只展示 profile，不允许用户或模型提交任意 endpoint/model。自动化测试通过依赖注入使用受控 mock，不修改生产配置。
3. 文档仍按结构化 chunk 保存到本地 SQLite。`knowledge_chunks` 是引用事实，SQLite FTS5/BM25 是词法派生索引，归一化 little-endian `f32` BLOB 是语义派生索引；Rust 在有界扫描内计算 cosine，并用 RRF 融合两路结果。
4. semantic 检索默认关闭。只有用户开启 semantic 并配置凭据后，才把已选 chunk 或当前 query 发送到 SiliconFlow；不发送绝对路径、环境变量、API Key、未选中文件或向量 BLOB。
5. 索引先生成完整 FTS 候选 revision，再按最多 32 条一批请求 embedding。成功响应必须校验 `object`、`model`、`data[].object`、连续且不重复的 `index`、向量维度和 finite 数值；不发送 `dimensions` 或使用 `truncate` 静默丢内容。
6. 429/503/504 和可判定网络超时最多指数退避重试 2 次；400/401/403/404、响应结构错误和取消不重试。已有 active revision 在实际 embedding 失败时继续提供服务；首次索引没有旧 revision 时才允许把完整 FTS revision 标为 `lexical_only` active。用户主动关闭 semantic 或没有 Key 时，按 lexical 模式更新 FTS active revision。
7. 未来增加 Qwen/VL、第二个远程服务或本地模型时，必须建立独立 profile、维度/输入能力矩阵、迁移和评测，禁止与 BGE-M3 向量混用。

## 影响

### 收益

- 不打包或维护本地 embedding 模型，第一期部署和磁盘成本较低。
- FTS5 在无 Key、限流、网络失败或用户关闭 semantic 时仍可用，普通对话不被外部服务阻塞。
- 向量与 chunk 同处本地 SQLite，删除来源、重建索引和备份边界明确；RRF 不需要专用向量数据库。
- 固定 endpoint/model 缩小 SSRF、能力漂移和向量混用风险，API 约束可由自动化夹具覆盖。

### 成本与限制

- semantic 索引和查询会产生第三方网络请求，免费额度、TPM 限制和服务可用性不作产品承诺。
- 本地 Rust cosine 扫描只适合第一期有界规模；超过扫描上限时只能返回 lexical-only，规模增长后需要专用索引评估。
- BGE-M3 维度以响应为准，不能通过 `dimensions` 请求参数指定；模型服务变更可能要求重新索引。
- API Key、chunk 正文和 query 的保留/删除策略必须继续遵守知识库设计文档的隐私和日志约束。

## 未采用方案

### 打包本地 embedding 模型

未采用，因为会增加安装包体积、内存、模型升级和跨平台运行时成本；第一期优先使用用户已选择的托管 API。

### PostgreSQL/pgvector 或独立向量数据库

未采用，因为当前是本地桌面单用户场景，引入服务端和运维组件的成本高于有界 SQLite 扫描收益；多用户共享和大规模索引留到后续阶段。

### 只使用远程向量检索

未采用，因为 FTS-only 降级、离线可用性和本地引用事实仍然是必要的安全与可用性边界。

### 直接接受用户提供的 endpoint 或 VL 输入

未采用，因为会扩大 SSRF、数据外发和能力矩阵风险；需要新的 provider/profile 契约后再单独评估。

## 相关文档

- [知识库开发设计](../知识库开发设计.md)
- [硅基流动创建嵌入请求](https://api-docs.siliconflow.cn/docs/api/embeddings-post)
