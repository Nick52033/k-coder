import readline from "node:readline";

const input = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });

for await (const line of input) {
  const message = JSON.parse(line);
  if (message.method === "notifications/initialized") continue;
  let result;
  if (message.method === "initialize") {
    if (process.env.TEST_SECRET !== "hidden-value") {
      process.stdout.write(`${JSON.stringify({ jsonrpc: "2.0", id: message.id, error: { code: -32000, message: "missing secret" } })}\n`);
      continue;
    }
    result = {
      protocolVersion: "2025-11-25",
      capabilities: { tools: {} },
      serverInfo: { name: "fixture", version: "1.0.0" },
    };
  } else if (message.method === "tools/list") {
    result = {
      tools: [{
        name: "Echo-Text",
        description: "Echo bounded text",
        inputSchema: {
          type: "object",
          properties: { text: { type: "string" } },
          required: ["text"],
          additionalProperties: false,
        },
        annotations: { readOnlyHint: true, openWorldHint: false },
      }],
    };
  } else if (message.method === "tools/call") {
    result = { content: [{ type: "text", text: `echo:${message.params.arguments.text}` }] };
  } else {
    process.stdout.write(`${JSON.stringify({ jsonrpc: "2.0", id: message.id, error: { code: -32601, message: "unknown method" } })}\n`);
    continue;
  }
  process.stdout.write(`${JSON.stringify({ jsonrpc: "2.0", id: message.id, result })}\n`);
}
