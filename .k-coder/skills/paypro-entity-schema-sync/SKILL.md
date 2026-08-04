---
name: "paypro-entity-schema-sync"
description: "Use in the paypro-platform-be repository when comparing C# entity classes with the payproplatform MySQL schema through the mysql-payproplatform MCP server, always generating a local Markdown comparison report after comparison, and asking whether to complete safe entity-code gaps."
triggers:
  - entity schema sync
  - Paypro实体同步
  - 实体数据库对比
risk: external
enabled: true
---

# Paypro Entity Schema Sync

Use this skill only inside the `paypro-platform-be` repository. It is project-specific because it depends on the configured `mysql-payproplatform` MCP server and the `payproplatform` database.

## Goal

Compare C# entity classes against MySQL table definitions, then:

- First produce a concise comparison result and generate a local Markdown comparison report.
- After the Markdown report is generated, ask the user whether to complete the code.
- If the user chooses yes, add safe missing entity properties using the repository's existing entity style.
- If the user chooses no, do not edit entity code; the local Markdown report is already available.
- If the entity and table differ significantly, do not guess. Report unresolved fields, type/nullability differences, and questions or risks.

## Required MCP

Always use the `mysql-payproplatform` MCP server for database metadata. Do not rely on remembered schema or stale local notes.

Useful MCP queries:

```sql
SELECT TABLE_NAME, TABLE_COMMENT
FROM INFORMATION_SCHEMA.TABLES
WHERE TABLE_SCHEMA = 'payproplatform'
ORDER BY TABLE_NAME;
```

```sql
SELECT COLUMN_NAME, COLUMN_TYPE, DATA_TYPE, IS_NULLABLE, COLUMN_KEY,
       COLUMN_DEFAULT, EXTRA, COLUMN_COMMENT, ORDINAL_POSITION
FROM INFORMATION_SCHEMA.COLUMNS
WHERE TABLE_SCHEMA = 'payproplatform'
  AND TABLE_NAME = '<table_name>'
ORDER BY ORDINAL_POSITION;
```

```sql
SELECT INDEX_NAME, NON_UNIQUE, SEQ_IN_INDEX, COLUMN_NAME
FROM INFORMATION_SCHEMA.STATISTICS
WHERE TABLE_SCHEMA = 'payproplatform'
  AND TABLE_NAME = '<table_name>'
ORDER BY INDEX_NAME, SEQ_IN_INDEX;
```

Use `SHOW CREATE TABLE <table_name>;` when a full DDL view is needed.

## Entity Discovery

First inspect the repository instead of assuming paths:

```bash
rg --files -g "*.cs"
rg -n "class .*(:|\\{|Entity<|AggregateRoot|FullAudited|Audited|IEntity|\\[Table\\()" src
rg -n "ToTable\\(|DbSet<|IEntityTypeConfiguration<" src
```

Identify candidate entities by these signals:

- Classes mapped with `[Table("...")]`.
- EF Core mapping in `OnModelCreating`, `IEntityTypeConfiguration<T>`, or `ToTable("...")`.
- Domain/entity base types such as `Entity<>`, `AggregateRoot<>`, `AuditedEntity<>`, `FullAuditedEntity<>`, or project-local base entity classes.
- Repository or application code that consistently treats the class as persisted data.

Prefer explicit table mapping over name inference. Only infer a table name from class name when there is a clear project convention.

## Comparison Rules

Normalize names before comparing:

- Treat exact PascalCase/camelCase matches as equal, for example `CreationTime` and `CreationTime`.
- Treat database column names with underscores as PascalCase equivalents when the project already uses that convention.
- Ignore navigation properties, computed/non-mapped properties, `[NotMapped]` members, and domain methods.
- Ignore `TenantId` in both directions. Do not include it in table-only/code-only/type/nullability differences, do not count it toward drift thresholds, and do not add or remove it unless the user explicitly asks for `TenantId`.
- Treat common audit and soft-delete fields as real columns when the table contains them: `CreatorNo`, `CreationTime`, `LastModifierNo`, `LastModificationTime`, `IsDeleted`, `DeleterNo`, `DeletionTime`.

Recommended C# type mapping:

| MySQL type | C# type |
|---|---|
| `bigint` | `long` |
| `int` | `int` |
| `smallint` | `short` |
| `tinyint(1)` | `bool` when comments/defaults clearly indicate boolean; otherwise `byte` or existing project enum style |
| `tinyint` | `byte` or an existing enum if the codebase already defines one |
| `decimal(p,s)` | `decimal` |
| `double` | `double` |
| `float` | `float` |
| `datetime`, `timestamp` | `DateTime` |
| `date` | `DateTime` unless the project uses `DateOnly` |
| `varchar`, `char`, `text`, `longtext` | `string` |
| `json` | `string` unless the existing entity uses a JSON type/converter |
| `binary`, `varbinary`, `blob` | `byte[]` |

Apply nullability from `IS_NULLABLE` and the project's nullable-reference-type style:

- Value types are nullable when the column is nullable, for example `long?`, `int?`, `decimal?`, `DateTime?`.
- Reference types should follow the existing file style. If nullable reference types are enabled and the file uses `string?`, preserve that style.
- Do not add `required`, default initializers, or validation attributes unless surrounding properties already use them for the same pattern.

## Drift Threshold

Use judgment, but default to this threshold:

- Small drift: table-only missing fields are no more than 5 after ignoring `TenantId`, all have direct scalar mappings, and there are no conflicting same-name type/nullability changes.
- Large drift: more than 5 table-only fields, missing keys/audit structure, many type conflicts, unclear table mapping, or evidence that the class represents a DTO/view rather than the table.

Do not edit entity code immediately after detecting drift. Always generate the local Markdown report, present the comparison result and report path, then ask for the user's decision.

If the user chooses code completion, implement only small/safe scalar additions. For large or ambiguous drift, do not guess; report the unresolved items after any safe additions.

If the user declines code completion, stop without changing entity code. Do not regenerate the report unless the user asks.

## Decision Gate

After completing the comparison, generate the local Markdown report before asking this question. Then, before editing entity code, ask a concise yes/no question:

```text
Markdown 对比报告已生成：<report-path>。是否补全代码？回复“是/yes”我将自动补全安全字段；回复“否/no”我将不修改代码。
```

Interpret `是`, `yes`, `y`, `补全`, `自动补全` as yes. Interpret `否`, `no`, `n`, `报告`, `md`, `markdown` as no.

Do not ask this question until the comparison result is available and the local Markdown report has been written. The question should summarize the number of entities/tables compared, the number of safe table-only fields found, and the report path.

## Edit Workflow

1. Confirm current git status before editing so user changes are not mistaken for your own.
2. Locate the entity class and table mapping.
3. Query `mysql-payproplatform` for the exact table columns and indexes.
4. Compare code properties to table columns.
5. Ignore `TenantId` during comparison and drift counting.
6. Generate the local Markdown comparison report.
   - Default path: `.codex/reports/paypro-entity-schema-diff-<yyyyMMdd-HHmmss>.md`.
   - Create `.codex/reports` if it does not exist.
   - Do not overwrite an existing report unless the user requested a specific path.
7. Present the comparison result, include the report path, and ask the Decision Gate question.
8. If the user chooses yes and the drift is small/safe:
   - Add only missing scalar properties.
   - Place new properties near related fields or in table order if the file follows table order.
   - Match local formatting, access modifiers, comments, attributes, and nullable style.
   - Add or update EF mapping only when the project explicitly maps each property or a column needs a non-conventional name/type.
9. If the user chooses yes but drift is large/ambiguous:
   - Do not patch ambiguous fields.
   - Patch only clearly safe scalar fields when doing so does not hide unresolved drift.
   - Report table-only columns, code-only properties, type/nullability differences, and recommended next action.
10. If the user chooses no:
   - Do not patch files.
   - Refer the user to the already-generated local Markdown report.
11. Build the project when code was changed and feasible:

```bash
dotnet build
```

If build cannot run, say why and mention the residual risk.

## Local Markdown Report

Always write a compact, readable local Markdown report after comparison. Escape entity/table/field comments safely.

Use `.codex/reports/paypro-entity-schema-diff-<yyyyMMdd-HHmmss>.md` by default. If the user asks for a specific path, use that path when it stays inside the workspace.

Structure the Markdown in this order:

1. Entity and table names compared.
2. Summary count: code properties, table columns, table-only, code-only, type differences.
3. Ignored fields section that states `TenantId` was ignored.
4. Table-only fields with MySQL type, nullable/default, and comment.
5. Code-only fields.
6. Type/nullability differences.
7. Suggested safe next step.

Keep the report focused. Avoid dumping all 50+ fields unless the user asks for full detail. In the final response, link to the local Markdown file and summarize only the highest-signal differences.

## Safety

- Never modify generated files, migrations, DTOs, or view models unless the user explicitly asks.
- Never change database schema from this skill.
- Never remove code properties solely because they are not present in the table.
- Never infer enum conversions without checking existing enum definitions and usage.
- Never overwrite unrelated user changes.
