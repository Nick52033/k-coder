---
name: systematic-debugging
description: Diagnose failures from reproducible evidence before changing code, then verify the smallest root-cause fix.
triggers: [debug failure, investigate bug, root cause, unexpected behavior]
risk: read
category: quality_review
enabled: true
---
# Systematic Debugging

Reproduce or inspect the failure first. Capture the exact input, observed output, environment, boundary, and earliest trustworthy divergence. Trace data and control flow across the relevant layers instead of patching the final symptom.

Form a specific hypothesis and test it with the smallest discriminating observation. Compare working and failing paths, including configuration, ordering, cancellation, persistence, and platform differences. Once evidence identifies the cause, add regression coverage and implement the narrowest coherent fix.

Do not stack speculative changes, suppress errors, loosen assertions, or claim a root cause without evidence. Preserve artifacts needed to reproduce the issue and state any untested environment explicitly.
