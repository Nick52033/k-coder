---
name: performance-testing
description: Plan and execute bounded performance checks with explicit workloads, metrics, baselines, and bottleneck evidence.
triggers: [performance test, load test, benchmark, latency test]
risk: read
category: testing
enabled: true
---
# Performance Testing

Define the workload model, environment, dataset, warm-up, concurrency, duration, and success thresholds before execution. Measure latency percentiles, throughput, errors, saturation, memory, CPU, I/O, and resource cleanup as relevant. Compare against a stable baseline or stated target.

Separate client, network, service, database, and UI rendering costs. Repeat enough to expose variance without launching uncontrolled load. Never run stress or load tests against production or an external service without explicit authorization. Report raw conditions and uncertainty with conclusions.
