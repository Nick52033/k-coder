---
name: api-testing
description: Verify API contracts, validation, authorization, persistence, idempotency, failures, and compatibility.
triggers: [api testing, integration api, endpoint test, contract test]
risk: read
category: testing
enabled: true
---
# API Testing

Derive tests from the actual schema and boundary implementation. Cover request validation, response shape, status and error codes, authentication and authorization, pagination, ordering, filtering, idempotency, concurrency, timeout, cancellation, retry, and backward compatibility as applicable.

Exercise the real boundary when practical and keep credentials out of fixtures and logs. Validate persistence and side effects, not only response text. For external APIs, use an approved test environment and bounded data; otherwise state that the integration remains unverified.
