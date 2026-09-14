---
name: sites-hosting
description: Prepare local site builds and deployment handoffs in k-Coder. Use an existing authorized deployment tool only when available; OpenAI Sites cloud hosting is not connected here.
---

# 网站构建与部署交接

本机没有原插件的 OpenAI Sites Apps 连接器，因此不能创建远程 Sites 项目、获取源码凭据、设置 D1/R2 或发布 OpenAI 托管站点。不要调用假定存在的 connector 工具，不能伪造 project_id 或网站链接。

1. 用 read_file/list_directory 查看项目 package.json 和已有部署配置；保留既有平台及框架。
2. 用 run_command 执行项目现有测试和 build，检查退出码、产物目录和构建错误。命令工作目录与输入输出均在当前工作区。
3. 启动项目本地预览，通过 browser_* 工具核对页面、链接及交互；记录当前未解决问题。生成说明放入 docs/。
4. 用户要求发布时，确认其选定平台的实际工具/CLI与授权是否可用；已有授权有效。存在有效工具时按真实 Schema 执行，权限、密钥、审批与取消仍由宿主管理。缺少连接器/登录时明确指出具体缺项并交付已验证的构建产物，不能把本地预览说成公网发布。
5. 只有实际部署操作返回可访问地址并经验证后，才报告发布成功。本次“适配并启用插件”不授权发布任何网站。

已有 .openai/hosting.json 只用于读取项目归属供用户交接，不改动远程资源。API Key、Token、授权 Header 和完整环境变量不能写入报告或日志；凭据使用宿主系统凭据设施。
