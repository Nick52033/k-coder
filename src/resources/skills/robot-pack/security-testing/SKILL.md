---
name: security-testing
description: Test trust boundaries, authorization, input handling, secrets, filesystem containment, and audit behavior.
triggers: [security test, threat model, authorization test, path traversal]
risk: read
category: testing
enabled: true
---
# Security Testing

Identify assets, actors, trust boundaries, entry points, and abuse cases. Test server-side authorization rather than model-supplied permission claims. Cover path normalization, symbolic links and junctions, injection, command construction, secret redaction, unsafe deserialization, resource exhaustion, race conditions, and audit completeness where relevant.

Use harmless bounded payloads in an approved environment. Never exfiltrate data, expose credentials, or probe third-party systems without explicit authorization. Findings must include evidence, impact, affected boundary, reproducibility, and a scoped remediation. Distinguish confirmed vulnerabilities from hypotheses.
