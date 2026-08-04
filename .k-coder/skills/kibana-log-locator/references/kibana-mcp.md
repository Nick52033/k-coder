# Kibana MCP Capability Map

Use these tools in this order unless the user already supplied a known `connection_id`, index, and trace ID.

## Discovery

- `kibana_list_instances`: list available Kibana connections. Required first step. If empty, MCP is loaded but no usable Kibana connection is available.
- `kibana_list_indices`: list Kibana Data Views or index patterns. Supports keyword filtering and pagination. Use to find log indices such as filebeat, logstash, application logs, APM, or service-specific patterns.
- `kibana_list_saved_searches`: list saved searches. Use when teams already maintain filtered views for app logs, exceptions, or environments.

## Document Queries

- `kibana_query`: query Elasticsearch/Kibana documents with `query` or `query_dsl`.
  - Supports `index` or `index_pattern`.
  - Supports `time_field`, `time_from`, `time_to`; default time field is `@timestamp`, default range is recent 15 minutes, maximum range is 7 days.
  - Supports pagination and up to 500 docs per page.
  - Use `source_includes` aggressively for fields like `@timestamp`, `message`, `log.level`, `level`, `service.name`, `app`, `env`, `trace.id`, `traceId`, `requestId`, `correlationId`, `span.id`, `logger`, `thread`, `exception.message`, `exception.stacktrace`, `error.stack_trace`.
- `kibana_run_saved_search`: execute a saved search and optionally override query, index, source fields, pagination, and time range. Result size is smaller, so use it for known curated searches.

## Aggregations

- `kibana_stats`: run Elasticsearch aggregations. Use this to:
  - Count errors by service, logger, level, exception class, or API path.
  - Discover populated trace fields with `terms` aggregations.
  - Confirm whether the error spike is isolated or widespread.

## Practical Query Shapes

Exact phrase:

```json
{
  "query": "\"unique error phrase\" OR \"ExceptionClassName\"",
  "time_from": "2026-06-24T10:00:00+08:00",
  "time_to": "2026-06-24T10:30:00+08:00",
  "source_includes": ["@timestamp", "message", "log.level", "service.name", "trace.id", "traceId", "requestId", "logger", "thread"]
}
```

Trace field fallback:

```json
{
  "query": "trace.id:\"abc\" OR traceId:\"abc\" OR requestId:\"abc\" OR correlationId:\"abc\"",
  "time_from": "now-30m",
  "time_to": "now"
}
```

Error grouping aggregation:

```json
{
  "aggs": {
    "by_service": { "terms": { "field": "service.name.keyword", "size": 20 } },
    "by_logger": { "terms": { "field": "logger.keyword", "size": 20 } }
  },
  "query": "log.level:ERROR OR level:ERROR",
  "time_from": "now-1h",
  "time_to": "now"
}
```

Adjust `.keyword` suffixes based on index mappings. If an aggregation field fails, retry with the unsuffixed or alternate field name.
