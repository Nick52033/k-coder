# 阶段三：人工确认后的改代码流程

**门禁**：仅当用户明确确认（「确认修复」「开始改代码」「按方案执行」）后执行。

## 1. 更新文档状态

打开处理文档，将 `状态` 改为 **修复中**。

## 2. 实施改动

- 严格按文档「建议改动」执行，**最小必要 diff**
- 遵循目标仓库规范（ABP 分层、SqlSugar 仓储、编码与注释约定）
- 禁止改动文档未列出的无关文件
- **保持文件原有编码**（GB2312/UTF-8 BOM 等勿擅自转换）

## 3. 测试

```bash
cd D:\ProjectCode\{repo}
dotnet test
```

- 有针对性测试类时优先跑过滤：`dotnet test --filter "FullyQualifiedName~{ClassName}"`
- 前端项目按仓库 README 执行 lint/build

## 4. 回写文档

在「修复记录」节填写：

- 状态 → **已修复待验证**
- 实际改动文件列表
- 测试命令与结果

## 5. 向用户汇报

```
Bug #{id} 代码已修改（待您验证）：
- 文档：{路径}
- 改动文件：{列表}
- 测试：{结果}

请本地验证后：
- **「验证通过」** → 自动进入阶段四（更新文档、commit/push、建 MR、禅道 resolve），见 [verify-passed-phase.md](verify-passed-phase.md)
- **「有问题」** → 说明现象，继续修复
```

## 6. 禁止事项（除非用户明确要求）

- 自动 `git commit` / `git push`
- 修改生产数据库
- 跳过测试直接声称已修复
- 在未经确认时进入本阶段

## 7. 验证通过后（阶段四，自动执行）

用户说 **「验证通过」** 时，由 `zentao-bug-fix` **自动执行阶段四**，无需再单独要求 commit/push/MR/禅道操作。

完整步骤见 [verify-passed-phase.md](verify-passed-phase.md)，概要：

1. 文档状态 → **已解决**
2. 各仓库 commit + push
3. 创建 MR/PR
4. 执行 **zentao-bug-resolve**（`Set-ZentaoBugResolved.ps1`）
