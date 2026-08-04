---
name: kibana-log-locator
description: Analyze production or test logs through Kibana MCP, correlate error snippets with trace/request IDs, reconstruct nearby log timelines, and locate likely source code files and lines in the current workspace. Use when the user provides an error log fragment, exception text, time window, request ID, trace ID, API path, user/order/business ID, or asks to find the code location behind a Kibana error.
triggers:
  - kibana log locator
  - Kibana日志定位
  - trace id
risk: external
enabled: true
---

# Kibana Log Locator

Use this skill to turn a rough error fragment into a focused debugging result: the matching Kibana log entries, the trace or request context around them, and the likely code location in the local repository.

## Workflow

1. Identify the query seed.
   - Extract exact phrases from the user's snippet: exception class, error code, API path, logger/class name, business ID, traceId/requestId/correlationId, and uncommon Chinese or English message text.
   - Ask for only the missing context that materially changes the search: approximate time, environment, service/app, or a trace/request ID. If the user gave enough signal, proceed without asking.
   - Do not print secrets, tokens, cookies, authorization headers, ID cards, phone numbers, or full payloads in the final answer.

2. Check Kibana MCP access.
   - Call `kibana_list_instances` first. Use the returned `connection_id`.
   - If it returns no connections, report that the MCP server is reachable but no Kibana connection is configured or accessible. Do not guess a `connection_id` as the final answer.
   - After choosing a connection, call `kibana_list_indices` and `kibana_list_saved_searches` to find relevant data views and saved filters.

3. Find the first matching error.
   - Prefer exact phrase search when the user supplied a log line.
   - Prefer ID search when the user supplied a `traceId`, `requestId`, `correlationId`, `spanId`, order number, policy number, tenant, or user ID.
   - Use `kibana_query` with a narrow time range first, then widen up to the tool's maximum window if needed.
   - Request only useful fields with `source_includes` to keep output readable.

4. Recover trace context.
   - Look for common fields: `trace.id`, `traceId`, `trace_id`, `request.id`, `requestId`, `correlationId`, `correlation_id`, `span.id`, `spanId`, `transaction.id`, `x-b3-traceid`, `traceparent`, `service.name`, `app`, `application`, `logger`, `thread`, `log.level`, `message`, `exception.*`.
   - If a trace/request ID is found, query around the error timestamp for all logs with that ID.
   - If no trace ID exists, reconstruct context from nearby logs sharing service/app, thread, user/business ID, API path, or logger.
   - Use `kibana_stats` terms aggregations to discover which trace-like fields are populated when field names are unknown.

5. Build a concise timeline.
   - Include the earliest suspicious entry, the direct error entry, upstream/downstream calls, retries, external dependency failures, and the final response log if present.
   - Keep timestamps, service names, levels, logger/class, trace/request ID, and one-line message summaries.
   - Separate confirmed evidence from inference.

6. Locate local code.
   - Search stack trace frames first: fully qualified class names, method names, file names, and line numbers.
   - Search logger names and message templates with `rg`, escaping regex metacharacters or using fixed-string search when needed.
   - Search API paths, controller/action names, error codes, enum values, and exception text.
   - For Java/.NET-style logs, map package/namespace plus class name to source paths before broad text search.
   - Open the smallest relevant files and inspect the exact method or branch that can emit the log or exception.

7. Report developer-facing findings.
   - Lead with the likely code location and the specific error.
   - Provide the trace/request ID, Kibana time window, and the exact search terms used.
   - Include the shortest useful log excerpt or paraphrase; avoid dumping large payloads.
   - State confidence: confirmed by stack trace, confirmed by log message match, or inferred from nearby timeline.
   - Suggest the next debugging action only when it follows directly from the evidence.

## Query Patterns

Use `references/kibana-mcp.md` for the MCP tool capability map and example query shapes.

For exact text:

```text
"unique error phrase" OR +"ExceptionClassName" OR traceId:"abc123"
```

For common error discovery:

```text
log.level:(ERROR OR FATAL) OR level:(ERROR OR FATAL) OR message:(Exception OR ERROR OR failed)
```

For trace context:

```text
trace.id:"<trace-id>" OR traceId:"<trace-id>" OR requestId:"<request-id>" OR correlationId:"<id>"
```

Prefer `query_dsl` when combining exact IDs, time filters, and multiple possible field names; prefer `query` for quick exploratory searches.

## Code Search Patterns

Run focused searches in the workspace:

```powershell
rg -n --fixed-strings "exact log message"
rg -n "ExceptionClassName|ErrorCode|methodName|/api/path"
rg -n "logger name or class name"
```

When a stack frame points to a generated, packaged, or deployed line number that does not match local code, still identify the nearest local method and mention the line mismatch as a build/source-map uncertainty.
