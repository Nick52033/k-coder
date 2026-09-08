---
name: dingtalk-document
description: Publish an approved document through a configured DingTalk MCP integration when explicitly authorized.
triggers: [dingtalk document, publish to dingtalk, dingtalk knowledge base, ding document]
risk: external
category: integration_automation
enabled: true
---
# DingTalk Document Delivery

Use only when the user explicitly requests publication and an enabled DingTalk MCP exposes the required operation. Review the final local content, target knowledge space, folder, title, and overwrite behavior before the external call. Send bounded content and return the resulting document identifier or link.

Never read, write, display, or persist App Secrets, access tokens, authorization headers, or webhook credentials. Do not fall back to curl, shell helpers, browser login automation, or guessed endpoints. If the configured MCP is unavailable or insufficient, stop and report the precise blocker; the Skill does not grant external permission.
