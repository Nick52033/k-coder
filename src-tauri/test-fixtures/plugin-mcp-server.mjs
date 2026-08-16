import path from "node:path";
import readline from "node:readline";

const expectedRoot = path.resolve(process.argv[2]);
const input = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });

for await (const line of input) {
  const message = JSON.parse(line);
  if (message.method === "notifications/initialized") continue;
  let result;
  if (message.method === "initialize") {
    const validEnvironment = process.env.TEST_SECRET === "hidden-value"
      && path.resolve(process.env.CODEX_PLUGIN_ROOT ?? "") === expectedRoot
      && path.resolve(process.cwd()) === expectedRoot;
    if (!validEnvironment) {
      process.stdout.write(`${JSON.stringify({
        jsonrpc: "2.0",
        id: message.id,
        error: { code: -32000, message: "invalid plugin launch environment" },
      })}\n`);
      continue;
    }
    result = {
      protocolVersion: "2025-11-25",
      capabilities: { tools: {} },
      serverInfo: { name: "plugin-fixture", version: "1.0.0" },
    };
  } else if (message.method === "tools/list") {
    result = {
      tools: [{
        name: "plugin_read",
        description: "Read plugin data",
        inputSchema: { type: "object", additionalProperties: false },
        annotations: { readOnlyHint: true, openWorldHint: false },
      }],
    };
  } else {
    process.stdout.write(`${JSON.stringify({
      jsonrpc: "2.0",
      id: message.id,
      error: { code: -32601, message: "unknown method" },
    })}\n`);
    continue;
  }
  process.stdout.write(`${JSON.stringify({ jsonrpc: "2.0", id: message.id, result })}\n`);
}
