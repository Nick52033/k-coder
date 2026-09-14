---
name: sites-building
description: Build sites for Sites. Use when creating or modifying a site, or when the project contains `.openai/hosting.json`.
---

## k-Coder 本地执行入口

使用 read_file、list_directory、search_repository、apply_patch/write_file 和 run_command 开发当前工作区的网站。先读取项目 package.json，沿用已有框架及脚本；新项目可使用本插件 templates 下的模板，但模板依赖仍需按用户任务安装并实际构建。文件参数相对当前工作区，生成 Markdown 放在 docs/。

通过 run_command 执行项目已有 build/test/dev 命令，服务就绪后使用 browser_navigate、browser_snapshot、browser_click、browser_type、browser_screenshot 验证。浏览器工具控制独立会话，不能声称修改了右侧预览 iframe。

本机没有 Sites Apps 连接器：下文 Sites 托管、D1/R2 创建、云环境变量、SIWC/OAuth 等段落仅用于已有相应平台的项目，不能在本机直接调用不存在的云工具。当前可以完成源码、测试、构建产物与本地预览；发布需要用户指定并授权实际可用的平台。不要创建虚假的 project_id、云地址或部署成功状态。


# Site Building

Use this skill to create or modify sites that fit Sites' runtime well.

## Build Contract

Sites hosts Cloudflare Worker-compatible sites.

- Build deployable Worker output as ES modules (`ESM`), not CommonJS (`CJS`).
- Prefer vinext and this skill's bundled templates unless the user explicitly
  wants a different starting point.
- Preserve the selected starter's layout instead of inventing a new shell.
- Keep site code within the chosen site surface.
- Keep logical storage declarations in `.openai/hosting.json`.
- Keep local `.env` and `.env.example` files aligned with runtime environment
  keys needed during development; hosted runtime values are managed by Sites
  instead of repo metadata.
- Let Sites own real Cloudflare resource creation and deployment wiring.

Use another Cloudflare Worker-compatible site layout only when the user explicitly
wants a different stack or the existing project already uses one. Refer to
Cloudflare's web application framework guides:
https://developers.cloudflare.com/workers/framework-guides/web-apps/

## Build Composition

Choose only the flows the requested site needs.

- Static or content-led site: use project setup, site shaping, and local validation.
- Durable product state: add structured persistence with D1.
- Device-local UI preferences only: browser storage is acceptable.
- Upload, media, or document site: add file storage with R2.
- Site with uploaded files plus searchable or relational metadata: use D1 and R2
  together.
- Internal workspace site that only needs current-user identity: use the
  forwarded authenticated-user headers.
- Public or externally authenticated site: add SIWC/OAuth authentication.

Each flow may also be used independently for an existing site, such as adding D1,
adding authentication, or validating a current build without rebuilding the
whole project.

## Starting a Project

When creating a new site:

1. Choose the starter before making structural changes.
2. Use `templates/vinext-starter` for most sites, including internal workspace
   sites that can rely on the forwarded authenticated-user headers.
3. Use `templates/auth-siwc-enabled` only when the site explicitly needs public
   sign-in, external OAuth, or SIWC.
4. Preserve the selected starter's file layout and extend it instead of
   replacing it with a new shell.

When modifying an existing site, preserve its current structure unless the user
explicitly asks for a larger rework.

## Shaping the Site

For new sites, default to a static or server-rendered vinext site unless the
product needs persistent state, file storage, authentication, or the user
explicitly wants a different stack.

- Use React and vinext simply. Avoid unnecessary client state.
- Build the first viewport around the product itself, not generic dashboard
  chrome or placeholder cards.
- Keep copy concrete and product-specific.
- Add full-stack features for the requested workflow, not speculative future use.
- Keep server boundaries narrow and product-driven.

## Choosing Persistence and Storage

When the user asks for storage, persistence, saved state, accounts, records,
history, progress, or data that should survive across sessions, default to
platform-backed persistence rather than browser-only storage.

Use D1 for persistent structured state that needs to survive page reloads or
sessions, especially when it represents product data rather than transient UI
state.

Typical D1 fits include:

- Users, profiles, settings, tasks, notes, posts, comments, scores, progress,
  leaderboards, and workflow state.
- Relational data that needs filtering, sorting, joins, indexing, ownership
  checks, or durable ids.
- Metadata for uploaded or generated files.

Use R2 for uploads, documents, images, videos, audio, exports, generated assets,
and other blobs.

Use D1 and R2 together when D1 stores metadata and R2 stores bytes, such as file
ownership, filenames, content type, processing status, or searchable fields.

Use browser storage only for device-local, non-authoritative UI preferences such
as dismissed banners, theme choice, or temporary draft state. Do not use
`localStorage`, `sessionStorage`, or in-memory state as the source of truth for
user data that the product is expected to remember.

Leave unused bindings `null`. Do not add persistence or object storage
speculatively, but when the product requires durable state, prefer platform
storage over browser-only storage.

## Adding Persistence and Storage

Use this flow after choosing the storage shape the product needs.

1. Set the needed logical bindings in `.openai/hosting.json`:
   - use `d1`, usually `DB`, when D1 is required.
   - use `r2` when R2 is required.
   - leave unused bindings `null`.
2. For D1-backed state:
   - put schema definitions in `db/schema.ts`.
   - keep D1 access behind a small helper instead of reading the runtime binding
     throughout route handlers.
   - generate and inspect Drizzle SQL after schema changes.
   - save generated migration files with the site source.
3. For R2-backed files:
   - keep large file payloads in R2 rather than D1.
   - store searchable, relational, or ownership metadata in D1 when the product
     needs it.
4. Keep the implementation tied to the requested product workflow rather than
   adding generic storage abstractions the site does not yet use.

Do not satisfy a durable-state request with `localStorage`, `sessionStorage`, or
in-memory state unless the user explicitly wants device-local behavior.

## Choosing Authentication

Choose the authentication model that matches where the site will run.

- For sites used inside an OpenAI workspace, prefer the platform-provided
  authenticated-user headers when the site only needs to know the current OpenAI
  user.
- For sites that need public sign-in, external identity providers, or explicit
  OAuth/SIWC flows, use the auth-enabled starter.
- Do not add a full auth stack when the workspace-authenticated user header is
  sufficient for the product.

Internal workspace sites can read the forwarded authenticated user email from the
`oai-authenticated-user-email` request header.

For SIWC-authenticated workspace sites, the dispatcher may also forward the
current user's non-empty SIWC `name` claim as
`oai-authenticated-user-full-name`. That value is percent-encoded UTF-8 and is
sent with an `oai-authenticated-user-full-name-encoding` header set to
`percent-encoded-utf-8`. Treat the full name as optional, decode it only when
the encoding header matches, and do not depend on name-split headers.

## Adding Authentication

Use this flow when the site needs identity-aware behavior.

1. Decide which auth model the product needs:
   - use the workspace-authenticated user header for internal OpenAI workspace
     sites when that identity is sufficient.
   - use `templates/auth-siwc-enabled` when the site needs public sign-in,
     external OAuth, or SIWC.
2. For workspace-authenticated sites:
   - read `oai-authenticated-user-email` from request headers where identity is
     needed.
   - read and decode `oai-authenticated-user-full-name` only when a display name
     improves the product, and always fall back to email because the full name
     header may be absent.
   - keep authorization decisions in server-side code.
3. For SIWC-enabled sites:
   - start from `templates/auth-siwc-enabled`.
   - keep auth wiring isolated from unrelated site code.
   - keep provider configuration in the starter's auth modules and environment
     variables instead of scattering it through route handlers or UI components.
4. Add authenticated product flows only where the site actually needs identity.

## Local Validation

Before calling the site work complete:

1. Run the site's normal build command, usually `npm run build`.
2. If the D1 schema changed, generate and inspect the local migration output.
3. Fix validation failures before continuing.

## Handoff to Hosting

After creating or modifying a site, finish local validation and use the
`sites-hosting` skill to deploy it, unless the user explicitly asks not to
deploy or to keep the work local.

Use `sites-hosting` for requests to save without deploying, deploy a saved
version, inspect versions, or manage production access.
